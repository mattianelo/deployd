use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::binary::{self, Reader};
use crate::core::game::mass_effect::Target;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Vector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LinearColor(pub f32, pub f32, pub f32, pub f32);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeadMorph {
    pub hair_mesh: String,
    pub accessory_mesh: Vec<String>,
    #[serde(with = "pairs")]
    pub morph_features: Vec<(String, f32)>,
    #[serde(with = "pairs")]
    pub offset_bones: Vec<(String, Vector)>,
    pub lod0_vertices: Vec<Vector>,
    pub lod1_vertices: Vec<Vector>,
    pub lod2_vertices: Vec<Vector>,
    pub lod3_vertices: Vec<Vector>,
    #[serde(with = "pairs")]
    pub scalar_parameters: Vec<(String, f32)>,
    #[serde(with = "pairs")]
    pub vector_parameters: Vec<(String, LinearColor)>,
    #[serde(with = "pairs")]
    pub texture_parameters: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct Preset {
    pub name: String,
    pub target: Option<Target>,
    pub morph: HeadMorph,
}

pub(crate) fn parse(bytes: &[u8], name: String) -> Result<Preset> {
    ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "Headmorph exceeds the 16 MiB limit"
    );
    let (target, morph) = if bytes.starts_with(b"GIBBEDMASSEFFECT") {
        let mut r = Reader::new(bytes);
        let magic = r.take(27)?;
        let target = match magic {
            b"GIBBEDMASSEFFECT2HEADMORPH\0" => Target::Le2,
            b"GIBBEDMASSEFFECT3HEADMORPH\0" => Target::Le3,
            _ => anyhow::bail!("Unsupported Gibbed headmorph header"),
        };
        ensure!(
            r.u32()? == if target == Target::Le2 { 29 } else { 59 },
            "Unsupported Gibbed headmorph version"
        );
        let morph = HeadMorph::read(&mut r)?;
        ensure!(r.position == bytes.len(), "Unexpected data after headmorph");
        (Some(target), morph)
    } else {
        let text = std::str::from_utf8(bytes)
            .context("Unsupported headmorph encoding; use TSE RON or a Gibbed preset")?;
        let morph: HeadMorph = ron::Options::default()
            .with_recursion_limit(32)
            .from_str(text)
            .context("Not a supported TSE headmorph preset")?;
        (None, morph)
    };
    morph.validate()?;
    Ok(Preset {
        name,
        target,
        morph,
    })
}

pub(crate) fn asset(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 4096
            && !value
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | ':')),
        "Use an Unreal object name, not a filesystem path or whitespace"
    );
    Ok(())
}

impl HeadMorph {
    pub fn validate(&self) -> Result<()> {
        asset(&self.hair_mesh)?;
        ensure!(
            self.accessory_mesh.len() <= 1024,
            "Too many accessory meshes"
        );
        for name in &self.accessory_mesh {
            asset(name)?;
        }
        fn keys<T>(values: &[(String, T)]) -> Result<()> {
            ensure!(values.len() <= 4096, "Too many headmorph parameters");
            let mut seen = BTreeSet::new();
            for (key, _) in values {
                ensure!(
                    !key.is_empty(),
                    "Appearance parameter names cannot be empty"
                );
                asset(key)?;
                ensure!(seen.insert(key), "Duplicate appearance parameter {key}");
            }
            Ok(())
        }
        keys(&self.morph_features)?;
        keys(&self.offset_bones)?;
        keys(&self.scalar_parameters)?;
        keys(&self.vector_parameters)?;
        keys(&self.texture_parameters)?;
        for (_, v) in self.morph_features.iter().chain(&self.scalar_parameters) {
            ensure!(v.is_finite(), "Appearance values must be finite");
        }
        for (_, v) in &self.vector_parameters {
            ensure!(
                [v.0, v.1, v.2, v.3].iter().all(|v| v.is_finite()),
                "Colors must contain finite values"
            );
        }
        for (_, v) in &self.texture_parameters {
            asset(v)?;
        }
        for lod in [
            &self.lod0_vertices,
            &self.lod1_vertices,
            &self.lod2_vertices,
            &self.lod3_vertices,
        ] {
            ensure!(lod.len() <= 100_000, "Headmorph contains too many vertices");
            for v in lod {
                ensure!(
                    [v.x, v.y, v.z].iter().all(|v| v.is_finite()),
                    "Invalid headmorph vertex"
                );
            }
        }
        for (_, v) in &self.offset_bones {
            ensure!(
                [v.x, v.y, v.z].iter().all(|v| v.is_finite()),
                "Invalid bone offset"
            );
        }
        Ok(())
    }

    pub fn export(&self) -> Result<String> {
        self.validate()?;
        Ok(ron::ser::to_string_pretty(
            self,
            ron::ser::PrettyConfig::default(),
        )?)
    }

    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self> {
        fn list<T>(
            r: &mut Reader<'_>,
            size: usize,
            mut read: impl FnMut(&mut Reader<'_>) -> Result<T>,
        ) -> Result<Vec<T>> {
            let n = r.count(size)?;
            (0..n).map(|_| read(r)).collect()
        }
        fn vector(r: &mut Reader<'_>) -> Result<Vector> {
            Ok(Vector {
                x: r.float()?,
                y: r.float()?,
                z: r.float()?,
            })
        }
        let result = Self {
            hair_mesh: r.string()?,
            accessory_mesh: list(r, 4, |r| r.string())?,
            morph_features: list(r, 8, |r| Ok((r.string()?, r.float()?)))?,
            offset_bones: list(r, 16, |r| Ok((r.string()?, vector(r)?)))?,
            lod0_vertices: list(r, 12, vector)?,
            lod1_vertices: list(r, 12, vector)?,
            lod2_vertices: list(r, 12, vector)?,
            lod3_vertices: list(r, 12, vector)?,
            scalar_parameters: list(r, 8, |r| Ok((r.string()?, r.float()?)))?,
            vector_parameters: list(r, 20, |r| {
                Ok((
                    r.string()?,
                    LinearColor(r.float()?, r.float()?, r.float()?, r.float()?),
                ))
            })?,
            texture_parameters: list(r, 8, |r| Ok((r.string()?, r.string()?)))?,
        };
        result.validate()?;
        Ok(result)
    }

    pub(super) fn write(&self, out: &mut Vec<u8>) {
        fn list<T>(out: &mut Vec<u8>, values: &[T], mut write: impl FnMut(&mut Vec<u8>, &T)) {
            binary::word(out, values.len() as u32);
            for value in values {
                write(out, value);
            }
        }
        fn vector(out: &mut Vec<u8>, v: &Vector) {
            for x in [v.x, v.y, v.z] {
                binary::word(out, x.to_bits());
            }
        }
        binary::string(out, &self.hair_mesh);
        list(out, &self.accessory_mesh, |o, v| binary::string(o, v));
        list(out, &self.morph_features, |o, (k, v)| {
            binary::string(o, k);
            binary::word(o, v.to_bits());
        });
        list(out, &self.offset_bones, |o, (k, v)| {
            binary::string(o, k);
            vector(o, v);
        });
        for lod in [
            &self.lod0_vertices,
            &self.lod1_vertices,
            &self.lod2_vertices,
            &self.lod3_vertices,
        ] {
            list(out, lod, vector);
        }
        list(out, &self.scalar_parameters, |o, (k, v)| {
            binary::string(o, k);
            binary::word(o, v.to_bits());
        });
        list(out, &self.vector_parameters, |o, (k, v)| {
            binary::string(o, k);
            for x in [v.0, v.1, v.2, v.3] {
                binary::word(o, x.to_bits());
            }
        });
        list(out, &self.texture_parameters, |o, (k, v)| {
            binary::string(o, k);
            binary::string(o, v);
        });
    }
}

mod pairs {
    use serde::{
        Deserialize, Deserializer, Serialize, Serializer,
        de::{Error, MapAccess, Visitor},
        ser::SerializeMap,
    };
    use std::{collections::BTreeSet, fmt, marker::PhantomData};
    pub fn serialize<S: Serializer, T: Serialize>(
        values: &[(String, T)],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(values.len()))?;
        for (k, v) in values {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Vec<(String, T)>, D::Error> {
        struct P<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for P<T> {
            type Value = Vec<(String, T)>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("appearance parameter map")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut result = Vec::new();
                let mut seen = BTreeSet::new();
                while let Some((k, v)) = map.next_entry::<String, T>()? {
                    if result.len() >= 4096 || !seen.insert(k.clone()) {
                        return Err(A::Error::custom(
                            "Too many or duplicate appearance parameters",
                        ));
                    }
                    result.push((k, v));
                }
                Ok(result)
            }
        }
        d.deserialize_map(P(PhantomData))
    }
}
