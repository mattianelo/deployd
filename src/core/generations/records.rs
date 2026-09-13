use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Column, Row, Sqlite, Transaction, TypeInfo, ValueRef};

pub(super) type Record = BTreeMap<String, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Table {
    Profiles,
    Groups,
    Mods,
    Files,
    Plugins,
    Masters,
    ProfileMods,
    ProfilePlugins,
    MelePackages,
    MeleRecipes,
}

impl Table {
    pub(super) const ALL: [Self; 10] = [
        Self::Profiles,
        Self::Groups,
        Self::Mods,
        Self::Files,
        Self::Plugins,
        Self::Masters,
        Self::ProfileMods,
        Self::ProfilePlugins,
        Self::MelePackages,
        Self::MeleRecipes,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Profiles => "profiles",
            Self::Groups => "mod_groups",
            Self::Mods => "mods",
            Self::Files => "mod_files",
            Self::Plugins => "plugins",
            Self::Masters => "plugin_masters",
            Self::ProfileMods => "profile_mods",
            Self::ProfilePlugins => "profile_plugins",
            Self::MelePackages => "mele_packages",
            Self::MeleRecipes => "mele_recipes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rows {
    pub table: Table,
    pub rows: Vec<Record>,
}

pub(super) async fn capture(
    tx: &mut Transaction<'_, Sqlite>,
    game: &str,
    profile: &str,
) -> Result<Vec<Rows>> {
    let mut result = Vec::new();
    let mods = "SELECT id FROM mods WHERE game_id = ?1 AND id IN (SELECT mod_id FROM profile_mods WHERE profile_id = ?2)";
    let plugins = format!("SELECT id FROM plugins WHERE mod_id IN ({mods})");
    for table in Table::ALL {
        let predicate = match table {
            Table::Profiles => "id = ?2 AND game_id = ?1".to_owned(),
            Table::Groups => "game_id = ?1 AND ?2 IS NOT NULL".to_owned(),
            Table::Mods => format!("id IN ({mods})"),
            Table::Files | Table::Plugins | Table::MelePackages => format!("mod_id IN ({mods})"),
            Table::Masters => format!("plugin_id IN ({plugins})"),
            Table::ProfileMods | Table::ProfilePlugins => {
                "profile_id = ?2 AND ?1 IS NOT NULL".to_owned()
            }
            Table::MeleRecipes => "game_id = ?1 AND profile_id = ?2".to_owned(),
        };
        let query = format!("SELECT * FROM {} WHERE {predicate}", table.name());
        let mut records = Vec::new();
        for row in sqlx::query(&query)
            .bind(game)
            .bind(profile)
            .fetch_all(&mut **tx)
            .await?
        {
            let mut record = Record::new();
            for column in row.columns() {
                let name = column.name();
                let raw = row.try_get_raw(name)?;
                let value = if raw.is_null() {
                    Value::Null
                } else {
                    match raw.type_info().name() {
                        "INTEGER" | "BOOLEAN" => Value::from(row.try_get::<i64, _>(name)?),
                        "REAL" => Value::from(row.try_get::<f64, _>(name)?),
                        "TEXT" => Value::from(row.try_get::<String, _>(name)?),
                        other => bail!("Unsupported generation metadata type {other} for {name}"),
                    }
                };
                record.insert(name.to_owned(), value);
            }
            if table == Table::Mods {
                record.insert("archive_path".into(), Value::Null);
            }
            records.push(record);
        }
        result.push(Rows {
            table,
            rows: records,
        });
    }
    ensure!(
        result[0].rows.len() == 1,
        "Generation profile no longer exists"
    );
    result[0].rows[0].insert("is_active".into(), Value::from(0));
    let mod_state = result
        .iter()
        .find(|rows| rows.table == Table::ProfileMods)
        .context("Profile mod state is absent")?
        .rows
        .iter()
        .map(|row| Ok((text(row, "mod_id")?.to_owned(), row.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let plugin_state = result
        .iter()
        .find(|rows| rows.table == Table::ProfilePlugins)
        .context("Profile plugin state is absent")?
        .rows
        .iter()
        .map(|row| Ok((text(row, "plugin_id")?.to_owned(), row.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    for rows in &mut result {
        let (states, order) = match rows.table {
            Table::Mods => (&mod_state, "priority"),
            Table::Plugins => (&plugin_state, "load_order"),
            _ => continue,
        };
        for row in &mut rows.rows {
            if let Some(state) = states.get(text(row, "id")?) {
                for column in ["enabled", order] {
                    row.insert(
                        column.into(),
                        state
                            .get(column)
                            .context("Profile ordering is incomplete")?
                            .clone(),
                    );
                }
            }
        }
    }
    for rows in &mut result {
        rows.rows.sort_by_cached_key(|row| format!("{row:?}"));
    }
    Ok(result)
}

pub(super) fn text<'a>(record: &'a Record, column: &str) -> Result<&'a str> {
    record
        .get(column)
        .and_then(Value::as_str)
        .with_context(|| format!("Generation metadata is missing {column}"))
}

pub(super) async fn insert(
    tx: &mut Transaction<'_, Sqlite>,
    table: Table,
    record: &Record,
) -> Result<()> {
    ensure!(!record.is_empty(), "Empty generation metadata record");
    let schema = sqlx::query(&format!("PRAGMA table_info({})", table.name()))
        .fetch_all(&mut **tx)
        .await?;
    for column in record.keys() {
        ensure!(
            schema
                .iter()
                .any(|row| row.get::<String, _>("name") == *column),
            "Unsupported historical metadata column {column}"
        );
    }
    let columns = record
        .keys()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = vec!["?"; record.len()].join(",");
    let query = format!(
        "INSERT INTO {} ({columns}) VALUES ({placeholders})",
        table.name()
    );
    let mut query = sqlx::query(&query);
    for value in record.values() {
        query = match value {
            Value::Null => query.bind(None::<String>),
            Value::String(value) => query.bind(value),
            Value::Number(value) if value.is_i64() => query.bind(value.as_i64()),
            Value::Number(value) => query.bind(value.as_f64()),
            Value::Bool(value) => query.bind(*value),
            _ => bail!("Invalid historical metadata value"),
        };
    }
    query
        .execute(&mut **tx)
        .await
        .with_context(|| format!("Cannot restore {} metadata", table.name()))?;
    Ok(())
}
