//! Configuration loading from TOML files.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub listen_addr: String,
    pub data_dir: PathBuf,
    pub session: SessionConfig,
    pub proxy: ProxyConfig,
    pub replay: ReplayConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub auto_create: bool,
    pub default_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub listen_addr: String,
    pub timeout_secs: u64,
    pub max_body_size: usize,
    pub capture_headers: bool,
    pub capture_bodies: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayConfig {
    pub delay_ms: u64,
    pub follow_redirects: bool,
    pub max_redirects: u32,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("ledger");

        Self {
            listen_addr: "127.0.0.1:8080".to_string(),
            data_dir,
            session: SessionConfig {
                auto_create: true,
                default_name: "default".to_string(),
            },
            proxy: ProxyConfig {
                listen_addr: "127.0.0.1:8080".to_string(),
                timeout_secs: 30,
                max_body_size: 10 * 1024 * 1024,
                capture_headers: true,
                capture_bodies: true,
            },
            replay: ReplayConfig {
                delay_ms: 0,
                follow_redirects: true,
                max_redirects: 10,
            },
        }
    }
}

pub fn load_config(path: &str) -> Result<Config> {
    let expanded = shellexpand(path);
    let config_path = PathBuf::from(expanded);

    if !config_path.exists() {
        return Ok(Config::default());
    }

    let contents = std::fs::read_to_string(&config_path)?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}

fn shellexpand(path: &str) -> String {
    if path.starts_with("~/")
        && let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), &path[1..]);
        }
    path.to_string()
}
