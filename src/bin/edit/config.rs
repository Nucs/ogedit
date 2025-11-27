// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Global configuration management for ogedit.
//!
//! Configuration is stored in `~/.ogedit/state.json`.

use std::fs;
use std::path::PathBuf;

use ogedit::apperr;

/// Global application configuration stored in ~/.ogedit/state.json
#[derive(Debug, Clone)]
pub struct Config {
    /// Whether word wrap is enabled by default for new documents
    pub word_wrap: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { word_wrap: false }
    }
}

impl Config {
    /// Load configuration from ~/.ogedit/state.json
    ///
    /// If the file doesn't exist or cannot be read, returns default configuration.
    /// If the file is corrupted, it will be backed up and a fresh config will be written.
    pub fn load() -> Self {
        let path = match Self::config_path() {
            Some(path) => path,
            None => return Default::default(),
        };

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => return Default::default(),
        };

        // Try to parse the config
        match Self::parse(&content) {
            Some(config) => config,
            None => {
                // File is corrupted, back it up and write a fresh one
                Self::handle_corruption(&path);
                Default::default()
            }
        }
    }

    /// Handle a corrupted config file by backing it up and writing a fresh one
    fn handle_corruption(path: &PathBuf) {
        // Create backup path: state.json -> state.json.backup
        let mut backup_path = path.clone();
        backup_path.set_file_name("state.json.backup");

        // Try to back up the corrupted file
        let _ = fs::rename(path, &backup_path);

        // Write a fresh config
        let default_config = Self::default();
        let _ = default_config.save();
    }

    /// Save configuration to ~/.ogedit/state.json
    pub fn save(&self) -> apperr::Result<()> {
        let path = match Self::config_path() {
            Some(path) => path,
            None => return Ok(()), // Can't save, but don't error
        };

        // Ensure the directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(apperr::Error::from)?;
        }

        let json = self.to_json();
        fs::write(&path, json).map_err(apperr::Error::from)?;

        Ok(())
    }

    /// Get the path to the configuration file (~/.ogedit/state.json)
    fn config_path() -> Option<PathBuf> {
        let home = if cfg!(windows) {
            std::env::var_os("USERPROFILE")?
        } else {
            std::env::var_os("HOME")?
        };

        let mut path = PathBuf::from(home);
        path.push(".ogedit");
        path.push("state.json");
        Some(path)
    }

    /// Parse configuration from JSON string
    fn parse(json: &str) -> Option<Self> {
        // Simple JSON parsing without dependencies
        // Format: {"word_wrap":true} or {"word_wrap":false}
        let json = json.trim();

        // Must be a JSON object
        if !json.starts_with('{') || !json.ends_with('}') {
            return None;
        }

        // Extract the content between braces
        let content = &json[1..json.len() - 1].trim();

        // Empty object is valid (will use defaults)
        if content.is_empty() {
            return Some(Self::default());
        }

        // Basic validation: if there's content, it should have quoted strings
        // This rejects obvious corruption like {invalid} or {123}
        if !content.is_empty() && !content.contains('"') {
            return None;
        }

        let mut word_wrap = false;

        // Extract word_wrap value
        // Look for "word_wrap" : true/false pattern
        if let Some(pos) = content.find("\"word_wrap\"") {
            let rest = &content[pos + "\"word_wrap\"".len()..].trim_start();

            // Must have a colon
            if !rest.starts_with(':') {
                return None;
            }

            let value_part = rest[1..].trim_start();

            if value_part.starts_with("true") {
                word_wrap = true;
            } else if value_part.starts_with("false") {
                word_wrap = false;
            } else {
                // Invalid value for word_wrap
                return None;
            }
        }

        // If we found something in the object but no valid word_wrap field,
        // it's either unknown fields (which we ignore) or malformed JSON
        // We'll accept it and use defaults for missing fields
        Some(Self { word_wrap })
    }

    /// Convert configuration to JSON string
    fn to_json(&self) -> String {
        format!("{{\"word_wrap\":{}}}\n", self.word_wrap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json() {
        let json = r#"{"word_wrap":true}"#;
        let config = Config::parse(json).unwrap();
        assert!(config.word_wrap);

        let json = r#"{"word_wrap":false}"#;
        let config = Config::parse(json).unwrap();
        assert!(!config.word_wrap);

        let json = r#"{"word_wrap": true }"#;
        let config = Config::parse(json).unwrap();
        assert!(config.word_wrap);
    }

    #[test]
    fn test_parse_json_corrupted() {
        // Invalid JSON should return None
        assert!(Config::parse("not json").is_none());
        assert!(Config::parse("{invalid}").is_none());
        assert!(Config::parse("").is_none());
        assert!(Config::parse("[]").is_none());

        // Incomplete JSON should return None
        assert!(Config::parse("{\"word_wrap\":").is_none());
        assert!(Config::parse("{\"word_wrap\"").is_none());
    }

    #[test]
    fn test_parse_json_missing_fields() {
        // Empty object should default to false
        let json = r#"{}"#;
        let config = Config::parse(json).unwrap();
        assert!(!config.word_wrap);

        // Unknown fields should be ignored
        let json = r#"{"unknown_field":123,"word_wrap":true}"#;
        let config = Config::parse(json).unwrap();
        assert!(config.word_wrap);
    }

    #[test]
    fn test_to_json() {
        let config = Config { word_wrap: true };
        assert_eq!(config.to_json(), "{\"word_wrap\":true}\n");

        let config = Config { word_wrap: false };
        assert_eq!(config.to_json(), "{\"word_wrap\":false}\n");
    }

    #[test]
    fn test_roundtrip() {
        // Test that we can serialize and deserialize correctly
        let original = Config { word_wrap: true };
        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed.word_wrap, original.word_wrap);

        let original = Config { word_wrap: false };
        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed.word_wrap, original.word_wrap);
    }
}
