// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! User-authored preferences are separate from automatically managed state.json.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::config::Config;

const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const MAX_ASSOCIATIONS: usize = 1024;

#[derive(Default)]
pub struct Settings {
    pub path: Option<PathBuf>,
    file_associations: Vec<(String, String)>,
    // Rendering asks more than once per frame. Keep only the active path so
    // custom globs run when switching files, not while typing or scrolling.
    active_match: RefCell<Option<(PathBuf, Option<usize>)>>,
}

impl Settings {
    pub fn new() -> Self {
        Self { path: Config::data_dir().map(|p| p.join("settings.json")), ..Self::default() }
    }

    /// Failed reloads retain the last working associations and the file itself.
    pub fn reload(&mut self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.file_associations.clear();
                self.active_match.get_mut().take();
                return Ok(());
            }
            Err(err) => return Err(err.to_string()),
        };
        let mut text = String::new();
        file.take(MAX_SETTINGS_BYTES + 1).read_to_string(&mut text).map_err(|e| e.to_string())?;
        let associations = Self::parse(&text)?;
        self.file_associations = associations;
        self.active_match.get_mut().take();
        Ok(())
    }

    fn parse(text: &str) -> Result<Vec<(String, String)>, String> {
        if text.len() as u64 > MAX_SETTINGS_BYTES {
            return Err("Settings exceed 1 MiB.".into());
        }
        crate::data_lens::check_json_depth(text)?;
        let parsed = jsonc_parser::parse_to_ast(
            text.trim_start_matches('\u{feff}'),
            &Default::default(),
            &crate::data_lens::json_options(true),
        )
        .map_err(|e| format!("Invalid settings JSON: {e}"))?;
        let root = parsed
            .value
            .as_ref()
            .and_then(|v| v.as_object())
            .ok_or("Settings must be a JSON object.")?;
        let mut result = Vec::new();
        if let Some(property) = root.get("files.associations") {
            let associations =
                property.value.as_object().ok_or("files.associations must be an object.")?;
            if associations.properties.len() > MAX_ASSOCIATIONS {
                return Err("At most 1,024 file associations are supported.".into());
            }
            for property in &associations.properties {
                let pattern = property.name.as_str();
                if pattern.is_empty() || pattern.len() > 4096 {
                    return Err("File association patterns must contain 1–4,096 bytes.".into());
                }
                let language = property
                    .value
                    .as_string_lit()
                    .ok_or("File association language IDs must be strings.")?;
                if !ogedit::syntax::language_exists(&language.value) {
                    return Err(format!("Unknown syntax language: {}", language.value));
                }
                let pattern = if pattern.contains('/') {
                    pattern.to_owned()
                } else {
                    format!("**/{pattern}")
                };
                result.push((pattern, language.value.to_string()));
            }
        }
        Ok(result)
    }

    pub fn language_for_path(&self, path: &Path) -> Option<&str> {
        if self.file_associations.is_empty() {
            return None;
        }
        if let Some((previous, index)) = &*self.active_match.borrow()
            && previous == path
        {
            return index.map(|index| self.file_associations[index].1.as_str());
        }
        let normalized = path.to_string_lossy().replace('\\', "/");
        let index = self.file_associations.iter().position(|(pattern, _)| {
            lsh::glob::glob_match(pattern.as_bytes(), normalized.as_bytes())
        });
        *self.active_match.borrow_mut() = Some((path.to_owned(), index));
        index.map(|index| self.file_associations[index].1.as_str())
    }

    pub fn is_settings_path(&self, path: &Path) -> bool {
        self.path.as_ref().is_some_and(|settings| {
            crate::safe_save::resolved_path(settings) == crate::safe_save::resolved_path(path)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn associations_keep_order_comments_path_globs_and_plaintext() {
        let text = r#"{
            // First matching rule wins, as in upstream.
            "files.associations": { "special/*.txt": "rust", "*.txt": "plaintext", "Dockerfile.*": "shellscript" }
        }"#;
        let settings =
            Settings { file_associations: Settings::parse(text).unwrap(), ..Settings::default() };
        assert_eq!(settings.language_for_path(Path::new("special/a.txt")), Some("rust"));
        assert_eq!(settings.language_for_path(Path::new("/else/a.txt")), Some("plaintext"));
        assert_eq!(
            settings.language_for_path(Path::new("/workspace/Dockerfile.dev")),
            Some("shellscript")
        );
        assert_eq!(settings.language_for_path(Path::new("a.rs")), None);
    }

    #[test]
    fn malformed_unknown_and_oversized_settings_are_rejected() {
        for text in [
            "[]",
            r#"{"files.associations":false}"#,
            r#"{"files.associations":{"*":1}}"#,
            r#"{"files.associations":{"*":"not-a-language"}}"#,
            r#"{"files.associations":{"":"rust"}}"#,
        ] {
            assert!(Settings::parse(text).is_err(), "{text}");
        }
        assert!(Settings::parse(&" ".repeat(MAX_SETTINGS_BYTES as usize + 1)).is_err());
        assert!(Settings::parse(&format!("{}{}", "[".repeat(200), "]".repeat(200))).is_err());
    }

    #[test]
    fn failed_reload_preserves_working_settings_and_original_file() {
        let root = std::env::temp_dir().join(format!(
            "ogedit-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        std::fs::write(&path, r#"{"files.associations":{"*.custom":"rust"}}"#).unwrap();
        let mut settings = Settings { path: Some(path.clone()), ..Settings::default() };
        settings.reload().unwrap();
        assert_eq!(settings.language_for_path(Path::new("a.custom")), Some("rust"));
        std::fs::write(&path, "{bad").unwrap();
        assert!(settings.reload().is_err());
        assert_eq!(settings.language_for_path(Path::new("a.custom")), Some("rust"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{bad");
        std::fs::write(&path, r#"{"files.associations":{"*.custom":"python"}}"#).unwrap();
        settings.reload().unwrap();
        assert_eq!(settings.language_for_path(Path::new("a.custom")), Some("python"));
        std::fs::remove_file(&path).unwrap();
        settings.reload().unwrap();
        assert_eq!(settings.language_for_path(Path::new("a.custom")), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
