use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::models::game::Game;
use crate::models::mod_entry::InstallTarget;

use super::engine_handler::EngineHandler;

pub(super) struct MassEffectHandler;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Target {
    #[serde(rename = "LE1")]
    Le1,
    #[serde(rename = "LE2")]
    Le2,
    #[serde(rename = "LE3")]
    Le3,
}

impl Target {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "LE1" => Ok(Self::Le1),
            "LE2" => Ok(Self::Le2),
            "LE3" => Ok(Self::Le3),
            _ => bail!("Expected a Legendary Edition game (LE1, LE2 or LE3), got '{value}'"),
        }
    }

    pub(super) fn is_official_dlc(self, name: &str) -> bool {
        let names: &[&str] = match self {
            Self::Le1 => &[],
            Self::Le2 => &[
                "DLC_00_Shared",
                "DLC_CER_01",
                "DLC_CER_02",
                "DLC_CER_Arc",
                "DLC_CON_Pack01",
                "DLC_CON_Pack02",
                "DLC_DHME1",
                "DLC_EXP_Part01",
                "DLC_EXP_Part02",
                "DLC_HEN_MT",
                "DLC_HEN_VT",
                "DLC_MCR_01",
                "DLC_MCR_03",
                "DLC_METR_Patch01",
                "DLC_PRE_Cerberus",
                "DLC_PRE_Collectors",
                "DLC_PRE_DA",
                "DLC_PRE_General",
                "DLC_PRE_Incisor",
                "DLC_PRE_Terminus",
                "DLC_PRO_Gulp01",
                "DLC_PRO_Pepper01",
                "DLC_PRO_Pepper02",
                "DLC_UNC_Hammer01",
                "DLC_UNC_Moment01",
                "DLC_UNC_Pack01",
                "DLC_UPD_Patch01",
                "DLC_UPD_Patch02",
                "DLC_UPD_Patch03",
            ],
            Self::Le3 => &[
                "DLC_CON_APP01",
                "DLC_CON_DH1",
                "DLC_CON_END",
                "DLC_HEN_PR",
                "DLC_CON_GUN01",
                "DLC_CON_GUN02",
                "DLC_CON_PRO1",
                "DLC_CON_PRO2",
                "DLC_CON_PRO3",
                "DLC_CON_PRO4",
                "DLC_CON_PRO5",
                "DLC_CON_PRO6",
                "DLC_EXP_Pack001",
                "DLC_EXP_Pack002",
                "DLC_EXP_Pack003",
                "DLC_EXP_Pack003_Base",
                "DLC_METR_Patch01",
                "DLC_OnlinePassHidCE",
                "DLC_UPD_Patch01",
                "DLC_UPD_Patch02",
            ],
        };
        names
            .iter()
            .any(|official| official.eq_ignore_ascii_case(name))
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Le1 => "Mass Effect 1 Legendary Edition",
            Self::Le2 => "Mass Effect 2 Legendary Edition",
            Self::Le3 => "Mass Effect 3 Legendary Edition",
        }
    }

    pub(crate) fn game_id(self) -> &'static str {
        match self {
            Self::Le1 => "mass-effect-le1",
            Self::Le2 => "mass-effect-le2",
            Self::Le3 => "mass-effect-le3",
        }
    }
}

impl EngineHandler for MassEffectHandler {
    fn validate_file_deployment(&self) -> Result<()> {
        bail!(
            "Mass Effect requires its restoration baseline and journaled installation; generic file deployment and purge are unavailable"
        )
    }

    fn route_file_list(
        &self,
        _game: &Game,
        _mod_name: &str,
        _stripped_wrapper: Option<&str>,
        _file_list: Vec<(PathBuf, PathBuf)>,
        _file_targets: &HashMap<String, InstallTarget>,
    ) -> Result<Vec<(PathBuf, PathBuf)>> {
        bail!(
            "Mass Effect packages require a restoration baseline and a validated transformation plan; generic file deployment cannot install them"
        )
    }
}
pub(crate) mod application;
pub(crate) mod baseline;
pub(crate) mod binary;
pub(crate) mod components;
pub(crate) mod family;
pub(crate) mod journal;
pub(crate) mod launcher;
pub(crate) mod library;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Recipe coordination awaits the in-app manifest installer"
    )
)]
pub(crate) mod recipe;

fn mutation_lock() -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCK: std::sync::OnceLock<std::sync::Arc<tokio::sync::Mutex<()>>> =
        std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

mod alternates;
mod dependency;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Helper supervision is exercised independently until journaled deployment is connected"
    )
)]
mod helper;
mod m3cd;
mod m3da;
mod m3gs;
mod m3m;
mod m3to;
mod m3za;
pub(crate) mod manifest;
mod merge_dlc;
mod merge_manifest;
mod operation;
pub(crate) mod package;
mod plot;
pub(crate) mod removal;
mod squad_movie;
