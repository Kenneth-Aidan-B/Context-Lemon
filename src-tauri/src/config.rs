use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub folders: Vec<String>,
    #[serde(default = "default_lemonade_url")]
    pub lemonade_url: String,
    /// The generation model answers are produced with. Defaulted rather than optional
    /// so a config written before the picker existed picks up the current default on
    /// load instead of deserialising into an empty model name.
    #[serde(default = "default_chat_model")]
    pub chat_model: String,
}

fn default_lemonade_url() -> String {
    "http://localhost:13305/v1".to_string()
}

fn default_chat_model() -> String {
    crate::rag::DEFAULT_CHAT_MODEL.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            folders: Vec::new(),
            lemonade_url: default_lemonade_url(),
            chat_model: default_chat_model(),
        }
    }
}

pub struct ConfigState(pub Mutex<Config>);

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    base.join("lemonade-context-engine")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn index_path() -> PathBuf {
    config_dir().join("index.bin")
}

pub fn load() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save(config: &Config) -> std::io::Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let raw = serde_json::to_string_pretty(config)?;
    fs::write(config_path(), raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every config.json written before the model picker existed lacks `chat_model`.
    /// Without a serde default those files would deserialize the field as empty and the
    /// app would ask Lemonade to generate with a model named "", so this is the upgrade
    /// path for existing installs rather than a formality.
    #[test]
    fn a_config_written_before_the_picker_existed_gains_the_default_model() {
        // The backslash is escaped for JSON, so the folder really is `D:\notes`. Written
        // unescaped, JSON would read `\n` as a newline and this would quietly assert on
        // a path that no filesystem could produce.
        let old = r#"{"folders":["D:\\notes"],"lemonade_url":"http://localhost:13305/v1"}"#;
        let config: Config = serde_json::from_str(old).expect("old configs must still load");
        assert_eq!(config.chat_model, crate::rag::DEFAULT_CHAT_MODEL);
        assert_eq!(config.folders, vec![r"D:\notes".to_string()]);
    }

    /// A config predating the URL field too must not lose the model default.
    #[test]
    fn a_minimal_config_gets_both_defaults() {
        let config: Config = serde_json::from_str(r#"{"folders":[]}"#).unwrap();
        assert_eq!(config.chat_model, crate::rag::DEFAULT_CHAT_MODEL);
        assert_eq!(config.lemonade_url, default_lemonade_url());
    }

    /// A model the user picked must survive a save/load cycle, or the choice would
    /// silently revert to the default on every restart.
    #[test]
    fn a_chosen_model_survives_a_round_trip() {
        let mut config = Config::default();
        config.chat_model = "Qwen3-1.7B-GGUF".to_string();
        let raw = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&raw).unwrap();
        assert_eq!(restored.chat_model, "Qwen3-1.7B-GGUF");
    }
}
