use serde::{Deserialize, Serialize};

pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/chat.rs"));
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub relay_adress: String,
    pub storage_path: Option<String>,
}

impl AppConfig {
    pub fn save_to_file(&self, path: &str) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
    }

    pub fn load_from_file(path: &str) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(path)?;
        let config = serde_json::from_str(&content)?;
        Ok(config)
    }
}
