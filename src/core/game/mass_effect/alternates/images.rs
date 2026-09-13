use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use gtk::gdk_pixbuf::prelude::*;

use super::super::package::PackagePlan;

pub(in crate::core::game::mass_effect) fn inspect(
    root: &Path,
    plan: &PackagePlan,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut result = BTreeMap::new();
    let prefix = super::prefix(plan)?;
    let names = plan
        .manifest
        .alternates
        .iter()
        .filter_map(|alt| alt.image.as_ref().map(|(name, _)| name))
        .chain(
            plan.manifest
                .metadata
                .get("modinfo.bannerimagename")
                .filter(|name| !name.is_empty()),
        );
    for name in names {
        if result.contains_key(name) {
            continue;
        }
        let name_path = super::super::manifest::relative_path(name)?;
        let path = format!("{prefix}M3Images/{name_path}");
        let source = plan
            .sources
            .iter()
            .find(|file| file.relative.eq_ignore_ascii_case(&path))
            .with_context(|| format!("Installer image '{name}' is missing"))?;
        let bytes = super::super::m3za::read_input(root, source, 16 * 1024 * 1024)?;
        let loader = gtk::gdk_pixbuf::PixbufLoader::new();
        loader.connect_size_prepared(|loader, width, height| {
            let ratio = f64::from(width.max(height).max(1)) / 512.0;
            if ratio > 1.0 {
                loader.set_size(
                    (f64::from(width) / ratio).max(1.0) as i32,
                    (f64::from(height) / ratio).max(1.0) as i32,
                );
            }
        });
        let decoded = loader.write(&bytes);
        let closed = loader.close();
        decoded.with_context(|| format!("Invalid installer image '{name}'"))?;
        closed?;
        let pixbuf = loader.pixbuf().context("Installer image has no pixels")?;
        ensure!(
            pixbuf.width() <= 512 && pixbuf.height() <= 512,
            "Installer image exceeds preview dimensions"
        );
        result.insert(name.clone(), pixbuf.save_to_bufferv("png", &[])?);
        ensure!(
            result.values().map(Vec::len).sum::<usize>() <= 32 * 1024 * 1024,
            "Installer image previews exceed the memory limit"
        );
    }
    Ok(result)
}
