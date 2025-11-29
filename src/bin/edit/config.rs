// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Global configuration management for ogedit.
//!
//! Configuration is stored in `~/.ogedit/state.json`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ogedit::apperr;

/// A recently opened file with timestamp
#[derive(Debug, Clone, PartialEq)]
pub struct RecentFile {
    /// Absolute path to the file
    pub path: PathBuf,
    /// Unix timestamp (seconds) when the file was last opened
    pub opened_at: u64,
}

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
    /// Per-project last-used save folder mapping (project_cwd -> last_save_dir)
    pub project_folders: HashMap<String, String>,
    /// Recently opened files (max 100), sorted by opened_at descending
    pub recent_files: Vec<RecentFile>,
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
            project_folders: HashMap::new(),
            recent_files: Vec::new(),
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

        // Parse project_folders HashMap
        config.project_folders = Self::parse_project_folders(content);

        // Parse recent_files array
        config.recent_files = Self::parse_recent_files(content);

        Some(config)
    }

    /// Parse a JSON object for project_folders: {"key": "value", ...}
    fn parse_project_folders(content: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();

        // Find "project_folders" field
        let Some(pos) = content.find("\"project_folders\"") else {
            return map;
        };

        let rest = content[pos + 17..].trim_start();
        if !rest.starts_with(':') {
            return map;
        }

        let value_part = rest[1..].trim_start();
        if !value_part.starts_with('{') {
            return map;
        }

        // Find the matching closing brace (accounting for strings)
        let mut depth = 0;
        let mut end_pos = 0;
        let mut in_string = false;
        let mut escape_next = false;
        for (i, c) in value_part.char_indices() {
            if escape_next {
                escape_next = false;
                continue;
            }
            match c {
                '\\' if in_string => escape_next = true,
                '"' => in_string = !in_string,
                '{' if !in_string => depth += 1,
                '}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        end_pos = i;
                        break;
                    }
                }
                _ => {}
            }
        }

        if end_pos == 0 {
            return map;
        }

        // Extract the object content between the braces
        let obj_content = &value_part[1..end_pos];

        // Parse key-value pairs: "key": "value"
        let mut remaining = obj_content;
        while let Some(key_start) = remaining.find('"') {
            remaining = &remaining[key_start + 1..];

            // Parse key (handle escapes)
            let (key, key_end) = Self::parse_json_string_content(remaining);
            if key_end == 0 {
                break;
            }
            remaining = &remaining[key_end + 1..];

            // Skip to colon
            remaining = remaining.trim_start();
            if !remaining.starts_with(':') {
                break;
            }
            remaining = &remaining[1..].trim_start();

            // Parse value
            if !remaining.starts_with('"') {
                break;
            }
            remaining = &remaining[1..];

            // Parse value (handle escapes)
            let (value, value_end) = Self::parse_json_string_content(remaining);
            if value_end == 0 {
                break;
            }
            remaining = &remaining[value_end + 1..];

            map.insert(key, value);
        }

        map
    }

    /// Parse a JSON string's content (after the opening quote), returning the unescaped string
    /// and the position of the closing quote
    fn parse_json_string_content(s: &str) -> (String, usize) {
        let mut result = String::new();
        let mut chars = s.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return (result, i),
                '\\' => {
                    if let Some((_, next)) = chars.next() {
                        match next {
                            'n' => result.push('\n'),
                            'r' => result.push('\r'),
                            't' => result.push('\t'),
                            '\\' => result.push('\\'),
                            '"' => result.push('"'),
                            '/' => result.push('/'),
                            _ => {
                                result.push('\\');
                                result.push(next);
                            }
                        }
                    }
                }
                _ => result.push(c),
            }
        }
        (result, 0) // No closing quote found
    }

    /// Parse a JSON array for recent_files: [{"path": "...", "opened_at": 123}, ...]
    fn parse_recent_files(content: &str) -> Vec<RecentFile> {
        let mut files = Vec::new();

        // Find "recent_files" field
        let Some(pos) = content.find("\"recent_files\"") else {
            return files;
        };

        let rest = content[pos + 14..].trim_start();
        if !rest.starts_with(':') {
            return files;
        }

        let value_part = rest[1..].trim_start();
        if !value_part.starts_with('[') {
            return files;
        }

        // Find the matching closing bracket (accounting for strings and nested objects)
        let mut depth = 0;
        let mut end_pos = 0;
        let mut in_string = false;
        let mut escape_next = false;
        for (i, c) in value_part.char_indices() {
            if escape_next {
                escape_next = false;
                continue;
            }
            match c {
                '\\' if in_string => escape_next = true,
                '"' => in_string = !in_string,
                '[' | '{' if !in_string => depth += 1,
                ']' | '}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        end_pos = i;
                        break;
                    }
                }
                _ => {}
            }
        }

        if end_pos == 0 {
            return files;
        }

        // Extract the array content between the brackets
        let array_content = &value_part[1..end_pos];

        // Parse each object in the array
        let mut remaining = array_content;
        while let Some(obj_start) = remaining.find('{') {
            remaining = &remaining[obj_start..];

            // Find matching closing brace
            let mut depth = 0;
            let mut obj_end = 0;
            let mut in_string = false;
            let mut escape_next = false;
            for (i, c) in remaining.char_indices() {
                if escape_next {
                    escape_next = false;
                    continue;
                }
                match c {
                    '\\' if in_string => escape_next = true,
                    '"' => in_string = !in_string,
                    '{' if !in_string => depth += 1,
                    '}' if !in_string => {
                        depth -= 1;
                        if depth == 0 {
                            obj_end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if obj_end == 0 {
                break;
            }

            let obj_content = &remaining[1..obj_end];

            // Parse "path" field
            let path = if let Some(path_pos) = obj_content.find("\"path\"") {
                let path_rest = obj_content[path_pos + 6..].trim_start();
                if path_rest.starts_with(':') {
                    let path_value = path_rest[1..].trim_start();
                    if path_value.starts_with('"') {
                        let (path_str, _) = Self::parse_json_string_content(&path_value[1..]);
                        Some(PathBuf::from(path_str))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // Parse "opened_at" field
            let opened_at = if let Some(at_pos) = obj_content.find("\"opened_at\"") {
                let at_rest = obj_content[at_pos + 11..].trim_start();
                if at_rest.starts_with(':') {
                    let at_value = at_rest[1..].trim_start();
                    let end = at_value.find(|c: char| !c.is_ascii_digit()).unwrap_or(at_value.len());
                    at_value[..end].parse::<u64>().ok()
                } else {
                    None
                }
            } else {
                None
            };

            if let (Some(path), Some(opened_at)) = (path, opened_at) {
                files.push(RecentFile { path, opened_at });
            }

            remaining = &remaining[obj_end + 1..];
        }

        files
    }

    /// Convert configuration to JSON string with comments
    fn to_json(&self) -> String {
        // Get platform-specific defaults for the comment
        let default_newline = if cfg!(windows) { "true" } else { "false" };
        let default_final_newline = if cfg!(windows) { "false" } else { "true" };

        // Build project_folders JSON object
        let project_folders_json = Self::project_folders_to_json(&self.project_folders);

        // Build recent_files JSON array
        let recent_files_json = Self::recent_files_to_json(&self.recent_files);

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
                "  \"ruler_column\": {},\n",
                "\n",
                "  // Per-project last-used save folder (auto-managed, do not edit)\n",
                "  \"project_folders\": {},\n",
                "\n",
                "  // Recently opened files (auto-managed, do not edit)\n",
                "  \"recent_files\": {}\n",
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
            self.ruler_column,
            project_folders_json,
            recent_files_json
        )
    }

    /// Convert project_folders HashMap to a JSON object string
    fn project_folders_to_json(map: &HashMap<String, String>) -> String {
        if map.is_empty() {
            return "{}".to_string();
        }

        let mut json = String::from("{\n");
        let mut first = true;
        for (key, value) in map {
            if !first {
                json.push_str(",\n");
            }
            first = false;
            json.push_str("    \"");
            json.push_str(&Self::escape_json_string(key));
            json.push_str("\": \"");
            json.push_str(&Self::escape_json_string(value));
            json.push('"');
        }
        json.push_str("\n  }");
        json
    }

    /// Convert recent_files Vec to a JSON array string
    fn recent_files_to_json(files: &[RecentFile]) -> String {
        if files.is_empty() {
            return "[]".to_string();
        }

        let mut json = String::from("[\n");
        let mut first = true;
        for file in files {
            if !first {
                json.push_str(",\n");
            }
            first = false;
            json.push_str("    { \"path\": \"");
            json.push_str(&Self::escape_json_string(&file.path.to_string_lossy()));
            json.push_str("\", \"opened_at\": ");
            json.push_str(&file.opened_at.to_string());
            json.push_str(" }");
        }
        json.push_str("\n  ]");
        json
    }

    /// Escape a string for JSON output
    fn escape_json_string(s: &str) -> String {
        let mut escaped = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                _ => escaped.push(c),
            }
        }
        escaped
    }

    /// Get the last-used save folder for a project, if any
    pub fn get_project_folder(&self, project_cwd: &str) -> Option<&str> {
        self.project_folders.get(project_cwd).map(|s| s.as_str())
    }

    /// Set the last-used save folder for a project and save config
    pub fn set_project_folder(&mut self, project_cwd: &str, last_save_dir: &str) {
        self.project_folders.insert(project_cwd.to_string(), last_save_dir.to_string());
        let _ = self.save();
    }

    /// Add or update a file in the recent files list.
    /// If the file already exists, updates its timestamp.
    /// Maintains max 100 entries, sorted by opened_at descending.
    pub fn add_recent_file(&mut self, path: &std::path::Path) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Check if file already exists in list
        if let Some(existing) = self.recent_files.iter_mut().find(|f| f.path == path) {
            existing.opened_at = now;
        } else {
            self.recent_files.push(RecentFile {
                path: path.to_path_buf(),
                opened_at: now,
            });
        }

        // Sort by opened_at descending (most recent first)
        self.recent_files.sort_by(|a, b| b.opened_at.cmp(&a.opened_at));

        // Trim to max 100 entries
        self.recent_files.truncate(100);

        let _ = self.save();
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
            project_folders: HashMap::new(),
            recent_files: Vec::new(),
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
    fn test_project_folders_roundtrip() {
        // Test that project_folders can be serialized and deserialized
        let mut original = Config::default();
        original.project_folders.insert("/home/user/project".to_string(), "/home/user/project/src".to_string());
        original.project_folders.insert("C:\\Users\\Test".to_string(), "C:\\Users\\Test\\Documents".to_string());

        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed.project_folders, original.project_folders);
    }

    #[test]
    fn test_project_folders_escaping() {
        // Test that special characters in paths are properly escaped and parsed
        let mut original = Config::default();
        original.project_folders.insert("path\"with\"quotes".to_string(), "value\\with\\backslashes".to_string());

        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed.project_folders, original.project_folders);
    }

    #[test]
    fn test_recent_files_roundtrip() {
        // Test that recent_files can be serialized and deserialized
        let mut original = Config::default();
        original.recent_files.push(RecentFile {
            path: PathBuf::from("/home/user/file.txt"),
            opened_at: 1732900000,
        });
        original.recent_files.push(RecentFile {
            path: PathBuf::from("C:\\Users\\Test\\doc.rs"),
            opened_at: 1732890000,
        });

        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed.recent_files, original.recent_files);
    }

    #[test]
    fn test_recent_files_escaping() {
        // Test that special characters in paths are properly escaped and parsed
        let mut original = Config::default();
        original.recent_files.push(RecentFile {
            path: PathBuf::from("path\"with\"quotes.txt"),
            opened_at: 1732900000,
        });
        original.recent_files.push(RecentFile {
            path: PathBuf::from("path\\with\\backslashes.txt"),
            opened_at: 1732890000,
        });

        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed.recent_files, original.recent_files);
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
