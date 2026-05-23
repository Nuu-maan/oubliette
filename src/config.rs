use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DISCORD_FREE_LIMIT: usize = 10 * 1024 * 1024;
pub const DEFAULT_CHUNK_TARGET: usize = 9 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bot_token: String,
    pub guild_id: u64,
    pub metadata_channel_id: u64,
    pub data_channel_ids: Vec<u64>,
    pub root_pointer_message_id: Option<u64>,

    #[serde(with = "hex::serde")]
    pub master_key: [u8; 32],

    #[serde(default = "default_chunk_target")]
    pub chunk_target: usize,
}

fn default_chunk_target() -> usize {
    DEFAULT_CHUNK_TARGET
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        let base = dirs::config_dir()
            .ok_or_else(|| Error::Config("no config dir".into()))?;
        Ok(base.join("oubliette").join("config.toml"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&raw)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        Ok(())
    }
}
