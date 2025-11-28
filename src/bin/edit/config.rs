// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Global configuration management for ogedit.
//!
//! Configuration is stored in `~/.ogedit/state.json`.

use std::fs;
use std::path::PathBuf;

use ogedit::apperr;

/// Global application configuration stored in ~/.ogedit/state.json
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Whether word wrap is enabled by default for new documents
    pub word_wrap: bool,
    /// Whether to use tabs (true) or spaces (false) for indentation
    pub indent_with_tabs: bool,
    /// Tab width / number of spaces for indentation (1-8)
    pub tab_size: u8,
    /// Whether to use CRLF (true) or LF (false) for newlines
    pub newline_crlf: bool,
    /// Whether to show line numbers in the left margin
    pub line_numbers: bool,
    /// Whether to highlight the current line
    pub line_highlight: bool,
    /// Whether to insert a final newline when saving
    pub insert_final_newline: bool,
    /// Ruler column position (0 = disabled, typically 80 or 120 for code)
    pub ruler_column: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            word_wrap: false,
            indent_with_tabs: false,  // Default to spaces
            tab_size: 4,              // Default to 4 spaces
            newline_crlf: cfg!(windows), // CRLF on Windows, LF elsewhere
            line_numbers: true,       // Show line numbers by default
            line_highlight: true,     // Highlight current line by default
            insert_final_newline: !cfg!(windows), // POSIX compliance on Unix
            ruler_column: 0,          // No ruler by default (0 = disabled)
        }
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

    /// Parse configuration from JSON string (supports // comments)
    fn parse(json: &str) -> Option<Self> {
        // Strip // comments (lines starting with //)
        let json: String = json
            .lines()
            .map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("//") {
                    "" // Remove comment lines entirely
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
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

        let defaults = Self::default();
        let mut config = defaults.clone();

        // Parse boolean field
        fn parse_bool(content: &str, field: &str, default: bool) -> Option<bool> {
            if let Some(pos) = content.find(&format!("\"{}\"", field)) {
                let rest = content[pos + field.len() + 2..].trim_start();
                if !rest.starts_with(':') {
                    return None;
                }
                let value_part = rest[1..].trim_start();
                if value_part.starts_with("true") {
                    return Some(true);
                } else if value_part.starts_with("false") {
                    return Some(false);
                } else {
                    return None; // Invalid value
                }
            }
            Some(default)
        }

        // Parse integer field
        fn parse_u8(content: &str, field: &str, default: u8, min: u8, max: u8) -> Option<u8> {
            if let Some(pos) = content.find(&format!("\"{}\"", field)) {
                let rest = content[pos + field.len() + 2..].trim_start();
                if !rest.starts_with(':') {
                    return None;
                }
                let value_part = rest[1..].trim_start();
                // Find the end of the number (digit characters)
                let end = value_part.find(|c: char| !c.is_ascii_digit()).unwrap_or(value_part.len());
                if end == 0 {
                    return None;
                }
                if let Ok(n) = value_part[..end].parse::<u8>() {
                    if n >= min && n <= max {
                        return Some(n);
                    }
                }
                return None; // Invalid value
            }
            Some(default)
        }

        config.word_wrap = parse_bool(content, "word_wrap", defaults.word_wrap)?;
        config.indent_with_tabs = parse_bool(content, "indent_with_tabs", defaults.indent_with_tabs)?;
        config.newline_crlf = parse_bool(content, "newline_crlf", defaults.newline_crlf)?;
        config.tab_size = parse_u8(content, "tab_size", defaults.tab_size, 1, 8)?;
        config.line_numbers = parse_bool(content, "line_numbers", defaults.line_numbers)?;
        config.line_highlight = parse_bool(content, "line_highlight", defaults.line_highlight)?;
        config.insert_final_newline = parse_bool(content, "insert_final_newline", defaults.insert_final_newline)?;
        config.ruler_column = parse_u8(content, "ruler_column", defaults.ruler_column, 0, 255)?;

        Some(config)
    }

    /// Convert configuration to JSON string with comments
    fn to_json(&self) -> String {
        // Get platform-specific defaults for the comment
        let default_newline = if cfg!(windows) { "true" } else { "false" };
        let default_final_newline = if cfg!(windows) { "false" } else { "true" };

        format!(
            concat!(
                "// OGEdit Configuration\n",
                "// Edit this file to customize your settings.\n",
                "// Lines starting with // are comments and will be ignored.\n",
                "{{\n",
                "  // Enable word wrap for long lines (true/false, default: false)\n",
                "  \"word_wrap\": {},\n",
                "\n",
                "  // Use tabs for indentation instead of spaces (true/false, default: false)\n",
                "  \"indent_with_tabs\": {},\n",
                "\n",
                "  // Number of spaces per tab/indentation level (1-8, default: 4)\n",
                "  \"tab_size\": {},\n",
                "\n",
                "  // Use Windows line endings CRLF (true) or Unix LF (false, default: {})\n",
                "  \"newline_crlf\": {},\n",
                "\n",
                "  // Show line numbers in left margin (true/false, default: true)\n",
                "  \"line_numbers\": {},\n",
                "\n",
                "  // Highlight the current line (true/false, default: true)\n",
                "  \"line_highlight\": {},\n",
                "\n",
                "  // Add newline at end of file when saving (true/false, default: {})\n",
                "  \"insert_final_newline\": {},\n",
                "\n",
                "  // Show vertical ruler at column, 0 to disable (0-255, default: 0)\n",
                "  \"ruler_column\": {}\n",
                "}}\n"
            ),
            self.word_wrap,
            self.indent_with_tabs,
            self.tab_size,
            default_newline,
            self.newline_crlf,
            self.line_numbers,
            self.line_highlight,
            default_final_newline,
            self.insert_final_newline,
            self.ruler_column
        )
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
    fn test_parse_json_all_fields() {
        let json = r#"{"word_wrap":true,"indent_with_tabs":true,"tab_size":2,"newline_crlf":false,"line_numbers":false,"line_highlight":false,"insert_final_newline":true,"ruler_column":80}"#;
        let config = Config::parse(json).unwrap();
        assert!(config.word_wrap);
        assert!(config.indent_with_tabs);
        assert_eq!(config.tab_size, 2);
        assert!(!config.newline_crlf);
        assert!(!config.line_numbers);
        assert!(!config.line_highlight);
        assert!(config.insert_final_newline);
        assert_eq!(config.ruler_column, 80);
    }

    #[test]
    fn test_parse_json_with_comments() {
        let json = r#"
// This is a comment
{
  // Another comment
  "word_wrap": true,
  // Comment about tabs
  "tab_size": 8
}
"#;
        let config = Config::parse(json).unwrap();
        assert!(config.word_wrap);
        assert_eq!(config.tab_size, 8);
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

        // Invalid tab_size values
        assert!(Config::parse("{\"tab_size\":0}").is_none());
        assert!(Config::parse("{\"tab_size\":9}").is_none());
        assert!(Config::parse("{\"tab_size\":abc}").is_none());
    }

    #[test]
    fn test_parse_json_missing_fields() {
        // Empty object should use defaults
        let json = r#"{}"#;
        let config = Config::parse(json).unwrap();
        let defaults = Config::default();
        assert_eq!(config, defaults);

        // Unknown fields should be ignored
        let json = r#"{"unknown_field":123,"word_wrap":true}"#;
        let config = Config::parse(json).unwrap();
        assert!(config.word_wrap);
    }

    #[test]
    fn test_roundtrip() {
        // Test that we can serialize and deserialize correctly
        let original = Config {
            word_wrap: true,
            indent_with_tabs: true,
            tab_size: 2,
            newline_crlf: false,
            line_numbers: false,
            line_highlight: false,
            insert_final_newline: true,
            ruler_column: 120,
        };
        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed, original);

        let original = Config::default();
        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_tab_size_bounds() {
        // Valid tab sizes
        for size in 1..=8 {
            let json = format!("{{\"tab_size\":{}}}", size);
            let config = Config::parse(&json).unwrap();
            assert_eq!(config.tab_size, size);
        }
    }

    #[test]
    fn test_ruler_column_bounds() {
        // Valid ruler columns (0 = disabled)
        for col in [0, 72, 80, 100, 120, 255] {
            let json = format!("{{\"ruler_column\":{}}}", col);
            let config = Config::parse(&json).unwrap();
            assert_eq!(config.ruler_column, col);
        }
    }
}
