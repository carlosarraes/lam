use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub server: String,
    pub token: String,
    pub topic: String,
    #[serde(default = "default_ntfy")]
    pub ntfy: String,
}

fn default_ntfy() -> String {
    "https://ntfy.sh".into()
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        if let Ok(p) = std::env::var("LAM_CONFIG") {
            return Ok(PathBuf::from(p));
        }
        Ok(dirs::config_dir().context("no config dir")?.join("lam").join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {} — run `lam init`", path.display()))?;
        Self::parse(&raw)
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let mut cfg: Config = toml::from_str(raw).context("invalid config")?;
        cfg.server = cfg.server.trim_end_matches('/').to_string();
        cfg.ntfy = cfg.ntfy.trim_end_matches('/').to_string();
        Ok(cfg)
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = Self::path()?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes() {
        let c = Config::parse("server = \"https://x.dev/\"\ntoken = \"t\"\ntopic = \"top\"\n").unwrap();
        assert_eq!(c.server, "https://x.dev");
        assert_eq!(c.ntfy, "https://ntfy.sh");
    }

    #[test]
    fn missing_field_fails() {
        assert!(Config::parse("server = \"x\"").is_err());
    }
}
