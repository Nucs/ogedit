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
use ogedit::input::{InputKey, kbmod, vk};

/// Keyboard shortcut configuration.
/// All shortcuts are stored as human-readable strings like "Ctrl+S", "Alt+Shift+F5", "F3".
#[derive(Debug, Clone, PartialEq)]
pub struct Hotkeys {
    /// File operations
    pub file_new: InputKey,
    pub file_open: InputKey,
    pub file_save: InputKey,
    pub file_save_as: InputKey,
    pub file_reload: InputKey,
    pub file_close: InputKey,
    pub file_exit: InputKey,

    /// Edit operations
    pub edit_undo: InputKey,
    pub edit_redo: InputKey,
    pub edit_cut: InputKey,
    pub edit_copy: InputKey,
    pub edit_paste: InputKey,
    pub edit_duplicate_line: InputKey,
    pub edit_find: InputKey,
    pub edit_replace: InputKey,
    pub edit_find_next: InputKey,
    pub edit_select_all: InputKey,

    /// View operations
    pub view_go_to_file: InputKey,
    pub view_go_to_line: InputKey,
    pub view_word_wrap: InputKey,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            // File operations
            file_new: kbmod::CTRL | vk::N,
            file_open: kbmod::CTRL | vk::O,
            file_save: kbmod::CTRL | vk::S,
            file_save_as: kbmod::CTRL_SHIFT | vk::S,
            file_reload: vk::F5,
            file_close: kbmod::CTRL | vk::W,
            file_exit: kbmod::CTRL | vk::Q,

            // Edit operations
            edit_undo: kbmod::CTRL | vk::Z,
            edit_redo: kbmod::CTRL | vk::Y,
            edit_cut: kbmod::CTRL | vk::X,
            edit_copy: kbmod::CTRL | vk::C,
            edit_paste: kbmod::CTRL | vk::V,
            edit_duplicate_line: kbmod::CTRL | vk::D,
            edit_find: kbmod::CTRL | vk::F,
            edit_replace: kbmod::CTRL | vk::R,
            edit_find_next: vk::F3,
            edit_select_all: kbmod::CTRL | vk::A,

            // View operations
            view_go_to_file: kbmod::CTRL | vk::P,
            view_go_to_line: kbmod::CTRL | vk::G,
            view_word_wrap: kbmod::ALT | vk::Z,
        }
    }
}

impl Hotkeys {
    /// Parse a hotkey string like "Ctrl+S", "Alt+Shift+F5", "F3" into an InputKey
    pub fn parse_hotkey(s: &str) -> Option<InputKey> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        let mut modifiers = kbmod::NONE;
        let mut key_part = s;

        // Parse modifiers (case-insensitive)
        loop {
            let lower = key_part.to_lowercase();
            if lower.starts_with("ctrl+") {
                modifiers |= kbmod::CTRL;
                key_part = &key_part[5..];
            } else if lower.starts_with("alt+") {
                modifiers |= kbmod::ALT;
                key_part = &key_part[4..];
            } else if lower.starts_with("shift+") {
                modifiers |= kbmod::SHIFT;
                key_part = &key_part[6..];
            } else {
                break;
            }
        }

        // Parse the key
        let key = Self::parse_key(key_part.trim())?;
        Some(key.with_modifiers(modifiers))
    }

    /// Parse a key name like "S", "F5", "Space", "Enter" into an InputKey
    fn parse_key(s: &str) -> Option<InputKey> {
        let lower = s.to_lowercase();
        let lower = lower.as_str();

        // Single letters A-Z
        if s.len() == 1 {
            let ch = s.chars().next()?;
            if ch.is_ascii_alphabetic() {
                let upper = ch.to_ascii_uppercase();
                return Some(InputKey::new(upper as u32));
            }
            // Single digits 0-9
            if ch.is_ascii_digit() {
                return Some(InputKey::new(ch as u32));
            }
        }

        // Function keys F1-F24
        if lower.starts_with('f') && lower.len() >= 2 {
            if let Ok(n) = lower[1..].parse::<u32>() {
                if (1..=24).contains(&n) {
                    return Some(InputKey::new(0x70 + n - 1)); // VK_F1 = 0x70
                }
            }
        }

        // Special keys
        match lower {
            "space" => Some(vk::SPACE),
            "enter" | "return" => Some(vk::RETURN),
            "tab" => Some(vk::TAB),
            "escape" | "esc" => Some(vk::ESCAPE),
            "backspace" | "back" => Some(vk::BACK),
            "delete" | "del" => Some(vk::DELETE),
            "insert" | "ins" => Some(vk::INSERT),
            "home" => Some(vk::HOME),
            "end" => Some(vk::END),
            "pageup" | "pgup" => Some(vk::PRIOR),
            "pagedown" | "pgdn" => Some(vk::NEXT),
            "up" => Some(vk::UP),
            "down" => Some(vk::DOWN),
            "left" => Some(vk::LEFT),
            "right" => Some(vk::RIGHT),
            _ => None,
        }
    }

    /// Convert an InputKey to a human-readable string like "Ctrl+S"
    pub fn hotkey_to_string(key: InputKey) -> String {
        let mut result = String::new();

        let modifiers = key.modifiers();
        if modifiers.contains(kbmod::CTRL) {
            result.push_str("Ctrl+");
        }
        if modifiers.contains(kbmod::ALT) {
            result.push_str("Alt+");
        }
        if modifiers.contains(kbmod::SHIFT) {
            result.push_str("Shift+");
        }

        let base_key = key.key();
        result.push_str(&Self::key_to_string(base_key));
        result
    }

    /// Convert a base key (without modifiers) to a string
    fn key_to_string(key: InputKey) -> String {
        let value = key.value();

        // Letters A-Z
        if (0x41..=0x5A).contains(&value) {
            return ((value as u8) as char).to_string();
        }

        // Digits 0-9
        if (0x30..=0x39).contains(&value) {
            return ((value as u8) as char).to_string();
        }

        // Function keys F1-F24
        if (0x70..=0x87).contains(&value) {
            return format!("F{}", value - 0x70 + 1);
        }

        // Special keys
        match value {
            0x20 => "Space".to_string(),
            0x0D => "Enter".to_string(),
            0x09 => "Tab".to_string(),
            0x1B => "Escape".to_string(),
            0x08 => "Backspace".to_string(),
            0x2E => "Delete".to_string(),
            0x2D => "Insert".to_string(),
            0x24 => "Home".to_string(),
            0x23 => "End".to_string(),
            0x21 => "PageUp".to_string(),
            0x22 => "PageDown".to_string(),
            0x26 => "Up".to_string(),
            0x28 => "Down".to_string(),
            0x25 => "Left".to_string(),
            0x27 => "Right".to_string(),
            _ => format!("0x{:02X}", value),
        }
    }
}

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
    /// Keyboard shortcuts (customizable)
    pub hotkeys: Hotkeys,
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
            hotkeys: Hotkeys::default(),
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

        // Parse hotkeys
        config.hotkeys = Self::parse_hotkeys(content, &defaults.hotkeys);

        Some(config)
    }

    /// Parse a hotkey string field, returning the default if not found or invalid
    fn parse_hotkey_field(content: &str, field: &str, default: InputKey) -> InputKey {
        if let Some(pos) = content.find(&format!("\"{}\"", field)) {
            let rest = content[pos + field.len() + 2..].trim_start();
            if let Some(after_colon) = rest.strip_prefix(':') {
                let value_part = after_colon.trim_start();
                if let Some(after_quote) = value_part.strip_prefix('"') {
                    let (value, _) = Self::parse_json_string_content(after_quote);
                    if let Some(key) = Hotkeys::parse_hotkey(&value) {
                        return key;
                    }
                }
            }
        }
        default
    }

    /// Parse the hotkeys object from JSON
    fn parse_hotkeys(content: &str, defaults: &Hotkeys) -> Hotkeys {
        // Find "hotkeys" field
        let Some(pos) = content.find("\"hotkeys\"") else {
            return defaults.clone();
        };

        let rest = content[pos + 9..].trim_start();
        if !rest.starts_with(':') {
            return defaults.clone();
        }

        let value_part = rest[1..].trim_start();
        if !value_part.starts_with('{') {
            return defaults.clone();
        }

        // Find the matching closing brace
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
            return defaults.clone();
        }

        let hotkeys_content = &value_part[1..end_pos];

        Hotkeys {
            file_new: Self::parse_hotkey_field(hotkeys_content, "file_new", defaults.file_new),
            file_open: Self::parse_hotkey_field(hotkeys_content, "file_open", defaults.file_open),
            file_save: Self::parse_hotkey_field(hotkeys_content, "file_save", defaults.file_save),
            file_save_as: Self::parse_hotkey_field(hotkeys_content, "file_save_as", defaults.file_save_as),
            file_reload: Self::parse_hotkey_field(hotkeys_content, "file_reload", defaults.file_reload),
            file_close: Self::parse_hotkey_field(hotkeys_content, "file_close", defaults.file_close),
            file_exit: Self::parse_hotkey_field(hotkeys_content, "file_exit", defaults.file_exit),

            edit_undo: Self::parse_hotkey_field(hotkeys_content, "edit_undo", defaults.edit_undo),
            edit_redo: Self::parse_hotkey_field(hotkeys_content, "edit_redo", defaults.edit_redo),
            edit_cut: Self::parse_hotkey_field(hotkeys_content, "edit_cut", defaults.edit_cut),
            edit_copy: Self::parse_hotkey_field(hotkeys_content, "edit_copy", defaults.edit_copy),
            edit_paste: Self::parse_hotkey_field(hotkeys_content, "edit_paste", defaults.edit_paste),
            edit_duplicate_line: Self::parse_hotkey_field(hotkeys_content, "edit_duplicate_line", defaults.edit_duplicate_line),
            edit_find: Self::parse_hotkey_field(hotkeys_content, "edit_find", defaults.edit_find),
            edit_replace: Self::parse_hotkey_field(hotkeys_content, "edit_replace", defaults.edit_replace),
            edit_find_next: Self::parse_hotkey_field(hotkeys_content, "edit_find_next", defaults.edit_find_next),
            edit_select_all: Self::parse_hotkey_field(hotkeys_content, "edit_select_all", defaults.edit_select_all),

            view_go_to_file: Self::parse_hotkey_field(hotkeys_content, "view_go_to_file", defaults.view_go_to_file),
            view_go_to_line: Self::parse_hotkey_field(hotkeys_content, "view_go_to_line", defaults.view_go_to_line),
            view_word_wrap: Self::parse_hotkey_field(hotkeys_content, "view_word_wrap", defaults.view_word_wrap),
        }
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
            let Some(after_colon) = remaining.strip_prefix(':') else {
                break;
            };
            remaining = after_colon.trim_start();

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
                if let Some(after_colon) = path_rest.strip_prefix(':') {
                    let path_value = after_colon.trim_start();
                    if let Some(after_quote) = path_value.strip_prefix('"') {
                        let (path_str, _) = Self::parse_json_string_content(after_quote);
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
                if let Some(after_colon) = at_rest.strip_prefix(':') {
                    let at_value = after_colon.trim_start();
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

        // Build hotkeys JSON object
        let hotkeys_json = Self::hotkeys_to_json(&self.hotkeys);

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
                "  // Keyboard shortcuts - customize using format like \"Ctrl+S\", \"Alt+Shift+F5\", \"F3\"\n",
                "  // Available modifiers: Ctrl, Alt, Shift (combine with +)\n",
                "  // Available keys: A-Z, 0-9, F1-F24, Space, Enter, Tab, Escape, Backspace, Delete,\n",
                "  //                 Insert, Home, End, PageUp, PageDown, Up, Down, Left, Right\n",
                "  \"hotkeys\": {},\n",
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
            hotkeys_json,
            project_folders_json,
            recent_files_json
        )
    }

    /// Convert Hotkeys to a JSON object string
    fn hotkeys_to_json(hotkeys: &Hotkeys) -> String {
        let mut json = String::from("{\n");

        // File operations
        json.push_str("    // File operations\n");
        json.push_str(&format!("    \"file_new\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.file_new)));
        json.push_str(&format!("    \"file_open\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.file_open)));
        json.push_str(&format!("    \"file_save\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.file_save)));
        json.push_str(&format!("    \"file_save_as\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.file_save_as)));
        json.push_str(&format!("    \"file_reload\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.file_reload)));
        json.push_str(&format!("    \"file_close\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.file_close)));
        json.push_str(&format!("    \"file_exit\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.file_exit)));

        // Edit operations
        json.push_str("\n    // Edit operations\n");
        json.push_str(&format!("    \"edit_undo\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_undo)));
        json.push_str(&format!("    \"edit_redo\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_redo)));
        json.push_str(&format!("    \"edit_cut\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_cut)));
        json.push_str(&format!("    \"edit_copy\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_copy)));
        json.push_str(&format!("    \"edit_paste\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_paste)));
        json.push_str(&format!("    \"edit_duplicate_line\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_duplicate_line)));
        json.push_str(&format!("    \"edit_find\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_find)));
        json.push_str(&format!("    \"edit_replace\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_replace)));
        json.push_str(&format!("    \"edit_find_next\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_find_next)));
        json.push_str(&format!("    \"edit_select_all\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.edit_select_all)));

        // View operations
        json.push_str("\n    // View operations\n");
        json.push_str(&format!("    \"view_go_to_file\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.view_go_to_file)));
        json.push_str(&format!("    \"view_go_to_line\": \"{}\",\n", Hotkeys::hotkey_to_string(hotkeys.view_go_to_line)));
        json.push_str(&format!("    \"view_word_wrap\": \"{}\"\n", Hotkeys::hotkey_to_string(hotkeys.view_word_wrap)));

        json.push_str("  }");
        json
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
            hotkeys: Hotkeys::default(),
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

    // ===== Hotkeys Tests =====

    #[test]
    fn test_parse_hotkey_simple() {
        // Simple letter keys
        assert_eq!(Hotkeys::parse_hotkey("S"), Some(InputKey::new(0x53)));
        assert_eq!(Hotkeys::parse_hotkey("A"), Some(InputKey::new(0x41)));
        assert_eq!(Hotkeys::parse_hotkey("Z"), Some(InputKey::new(0x5A)));

        // Digits
        assert_eq!(Hotkeys::parse_hotkey("0"), Some(InputKey::new(0x30)));
        assert_eq!(Hotkeys::parse_hotkey("9"), Some(InputKey::new(0x39)));
    }

    #[test]
    fn test_parse_hotkey_function_keys() {
        assert_eq!(Hotkeys::parse_hotkey("F1"), Some(InputKey::new(0x70)));
        assert_eq!(Hotkeys::parse_hotkey("F3"), Some(InputKey::new(0x72)));
        assert_eq!(Hotkeys::parse_hotkey("F5"), Some(InputKey::new(0x74)));
        assert_eq!(Hotkeys::parse_hotkey("F12"), Some(InputKey::new(0x7B)));
        assert_eq!(Hotkeys::parse_hotkey("F24"), Some(InputKey::new(0x87)));

        // Invalid function keys
        assert_eq!(Hotkeys::parse_hotkey("F0"), None);
        assert_eq!(Hotkeys::parse_hotkey("F25"), None);
    }

    #[test]
    fn test_parse_hotkey_special_keys() {
        assert!(Hotkeys::parse_hotkey("Space").is_some());
        assert!(Hotkeys::parse_hotkey("Enter").is_some());
        assert!(Hotkeys::parse_hotkey("Return").is_some());
        assert!(Hotkeys::parse_hotkey("Tab").is_some());
        assert!(Hotkeys::parse_hotkey("Escape").is_some());
        assert!(Hotkeys::parse_hotkey("Esc").is_some());
        assert!(Hotkeys::parse_hotkey("Backspace").is_some());
        assert!(Hotkeys::parse_hotkey("Delete").is_some());
        assert!(Hotkeys::parse_hotkey("Insert").is_some());
        assert!(Hotkeys::parse_hotkey("Home").is_some());
        assert!(Hotkeys::parse_hotkey("End").is_some());
        assert!(Hotkeys::parse_hotkey("PageUp").is_some());
        assert!(Hotkeys::parse_hotkey("PageDown").is_some());
        assert!(Hotkeys::parse_hotkey("Up").is_some());
        assert!(Hotkeys::parse_hotkey("Down").is_some());
        assert!(Hotkeys::parse_hotkey("Left").is_some());
        assert!(Hotkeys::parse_hotkey("Right").is_some());
    }

    #[test]
    fn test_parse_hotkey_with_modifiers() {
        // Ctrl+Key
        let key = Hotkeys::parse_hotkey("Ctrl+S").unwrap();
        assert!(key.modifiers().contains(kbmod::CTRL));
        assert_eq!(key.key().value(), 0x53); // 'S'

        // Alt+Key
        let key = Hotkeys::parse_hotkey("Alt+Z").unwrap();
        assert!(key.modifiers().contains(kbmod::ALT));
        assert_eq!(key.key().value(), 0x5A); // 'Z'

        // Shift+Key
        let key = Hotkeys::parse_hotkey("Shift+A").unwrap();
        assert!(key.modifiers().contains(kbmod::SHIFT));
        assert_eq!(key.key().value(), 0x41); // 'A'

        // Ctrl+Shift+Key
        let key = Hotkeys::parse_hotkey("Ctrl+Shift+S").unwrap();
        assert!(key.modifiers().contains(kbmod::CTRL));
        assert!(key.modifiers().contains(kbmod::SHIFT));
        assert_eq!(key.key().value(), 0x53); // 'S'

        // Alt+Shift+Key
        let key = Hotkeys::parse_hotkey("Alt+Shift+F5").unwrap();
        assert!(key.modifiers().contains(kbmod::ALT));
        assert!(key.modifiers().contains(kbmod::SHIFT));
        assert_eq!(key.key().value(), 0x74); // F5
    }

    #[test]
    fn test_parse_hotkey_case_insensitive() {
        // Modifiers should be case-insensitive
        let key1 = Hotkeys::parse_hotkey("ctrl+s").unwrap();
        let key2 = Hotkeys::parse_hotkey("CTRL+S").unwrap();
        let key3 = Hotkeys::parse_hotkey("Ctrl+S").unwrap();
        assert_eq!(key1, key2);
        assert_eq!(key2, key3);

        // Function keys too
        let key1 = Hotkeys::parse_hotkey("f5").unwrap();
        let key2 = Hotkeys::parse_hotkey("F5").unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_parse_hotkey_invalid() {
        assert_eq!(Hotkeys::parse_hotkey(""), None);
        assert_eq!(Hotkeys::parse_hotkey("   "), None);
        assert_eq!(Hotkeys::parse_hotkey("Invalid"), None);
        assert_eq!(Hotkeys::parse_hotkey("Ctrl+"), None);
        assert_eq!(Hotkeys::parse_hotkey("Ctrl+Invalid"), None);
    }

    #[test]
    fn test_hotkey_to_string() {
        // Simple keys
        assert_eq!(Hotkeys::hotkey_to_string(InputKey::new(0x53)), "S");
        assert_eq!(Hotkeys::hotkey_to_string(InputKey::new(0x30)), "0");
        assert_eq!(Hotkeys::hotkey_to_string(InputKey::new(0x74)), "F5");

        // With modifiers
        assert_eq!(Hotkeys::hotkey_to_string(kbmod::CTRL | vk::S), "Ctrl+S");
        assert_eq!(Hotkeys::hotkey_to_string(kbmod::ALT | vk::Z), "Alt+Z");
        assert_eq!(Hotkeys::hotkey_to_string(kbmod::CTRL_SHIFT | vk::S), "Ctrl+Shift+S");
    }

    #[test]
    fn test_hotkey_roundtrip() {
        // Test that parsing and serializing produces the same result
        let test_cases = [
            "Ctrl+S",
            "Ctrl+Shift+S",
            "Alt+Z",
            "F5",
            "F3",
            "Ctrl+N",
            "Ctrl+W",
            "Ctrl+P",
            "Ctrl+G",
        ];

        for original in test_cases {
            let parsed = Hotkeys::parse_hotkey(original).unwrap();
            let serialized = Hotkeys::hotkey_to_string(parsed);
            assert_eq!(serialized, original, "Roundtrip failed for: {}", original);
        }
    }

    #[test]
    fn test_hotkeys_config_roundtrip() {
        // Test that hotkeys survive config serialization/deserialization
        let mut config = Config::default();
        config.hotkeys.file_save = kbmod::CTRL_SHIFT | vk::S; // Custom hotkey
        config.hotkeys.edit_duplicate_line = kbmod::CTRL_SHIFT | vk::D;

        let json = config.to_json();
        let parsed = Config::parse(&json).unwrap();

        assert_eq!(parsed.hotkeys.file_save, config.hotkeys.file_save);
        assert_eq!(parsed.hotkeys.edit_duplicate_line, config.hotkeys.edit_duplicate_line);
    }

    #[test]
    fn test_hotkeys_partial_config() {
        // Test that missing hotkeys use defaults
        let json = r#"{"hotkeys": {"file_save": "Ctrl+Alt+S"}}"#;
        let config = Config::parse(json).unwrap();

        // Custom value
        let expected = kbmod::CTRL_ALT | vk::S;
        assert_eq!(config.hotkeys.file_save, expected);

        // Default values
        assert_eq!(config.hotkeys.file_new, Hotkeys::default().file_new);
        assert_eq!(config.hotkeys.edit_undo, Hotkeys::default().edit_undo);
    }

    #[test]
    fn test_hotkeys_invalid_value_uses_default() {
        // Invalid hotkey values should fall back to default
        let json = r#"{"hotkeys": {"file_save": "InvalidKey"}}"#;
        let config = Config::parse(json).unwrap();
        assert_eq!(config.hotkeys.file_save, Hotkeys::default().file_save);
    }

    // ===== Recent Files Tests =====

    #[test]
    fn test_add_recent_file_new() {
        let mut config = Config::default();
        assert!(config.recent_files.is_empty());

        config.recent_files.push(RecentFile {
            path: PathBuf::from("/test/file.txt"),
            opened_at: 1000,
        });

        assert_eq!(config.recent_files.len(), 1);
        assert_eq!(config.recent_files[0].path, PathBuf::from("/test/file.txt"));
    }

    #[test]
    fn test_recent_files_sorting() {
        let mut config = Config::default();

        config.recent_files.push(RecentFile {
            path: PathBuf::from("/old.txt"),
            opened_at: 1000,
        });
        config.recent_files.push(RecentFile {
            path: PathBuf::from("/new.txt"),
            opened_at: 2000,
        });
        config.recent_files.push(RecentFile {
            path: PathBuf::from("/middle.txt"),
            opened_at: 1500,
        });

        // Sort by opened_at descending
        config.recent_files.sort_by(|a, b| b.opened_at.cmp(&a.opened_at));

        assert_eq!(config.recent_files[0].path, PathBuf::from("/new.txt"));
        assert_eq!(config.recent_files[1].path, PathBuf::from("/middle.txt"));
        assert_eq!(config.recent_files[2].path, PathBuf::from("/old.txt"));
    }

    #[test]
    fn test_recent_files_max_limit() {
        let mut config = Config::default();

        // Add 105 files
        for i in 0..105 {
            config.recent_files.push(RecentFile {
                path: PathBuf::from(format!("/file{}.txt", i)),
                opened_at: i as u64,
            });
        }

        // Truncate to 100
        config.recent_files.truncate(100);

        assert_eq!(config.recent_files.len(), 100);
    }

    // ===== Project Folders Tests =====

    #[test]
    fn test_project_folder_get_set() {
        let mut config = Config::default();

        assert!(config.get_project_folder("/project1").is_none());

        config.project_folders.insert("/project1".to_string(), "/project1/src".to_string());

        assert_eq!(config.get_project_folder("/project1"), Some("/project1/src"));
        assert!(config.get_project_folder("/project2").is_none());
    }

    #[test]
    fn test_project_folder_multiple_projects() {
        let mut config = Config::default();

        config.project_folders.insert("/project1".to_string(), "/project1/src".to_string());
        config.project_folders.insert("/project2".to_string(), "/project2/docs".to_string());

        assert_eq!(config.get_project_folder("/project1"), Some("/project1/src"));
        assert_eq!(config.get_project_folder("/project2"), Some("/project2/docs"));
    }
}
