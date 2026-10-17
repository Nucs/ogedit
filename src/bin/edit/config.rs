// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Global configuration management for ogedit.
//!
//! Configuration is stored in `~/.ogedit/state.json`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
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
        let mut parts = s.trim().split('+');
        let key = Self::parse_key(parts.next_back()?.trim())?;
        let mut modifiers = kbmod::NONE;
        for part in parts {
            let modifier = match part.trim() {
                value if value.eq_ignore_ascii_case("ctrl") => kbmod::CTRL,
                value if value.eq_ignore_ascii_case("alt") => kbmod::ALT,
                value if value.eq_ignore_ascii_case("shift") => kbmod::SHIFT,
                _ => return None,
            };
            if modifiers.contains(modifier) {
                return None;
            }
            modifiers |= modifier;
        }
        // Terminals report these as text, so they cannot be reliable shortcuts.
        let printable = key == vk::SPACE
            || (0x30..=0x39).contains(&key.value())
            || (0x41..=0x5A).contains(&key.value());
        if printable && !modifiers.contains(kbmod::CTRL) && !modifiers.contains(kbmod::ALT) {
            return None;
        }
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
        if lower.starts_with('f')
            && lower.len() >= 2
            && let Ok(n) = lower[1..].parse::<u32>()
            && (1..=24).contains(&n)
        {
            return Some(InputKey::new(0x70 + n - 1)); // VK_F1 = 0x70
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
            indent_with_tabs: false,              // Default to spaces
            tab_size: 4,                          // Default to 4 spaces
            newline_crlf: cfg!(windows),          // CRLF on Windows, LF elsewhere
            line_numbers: true,                   // Show line numbers by default
            line_highlight: true,                 // Highlight current line by default
            insert_final_newline: !cfg!(windows), // POSIX compliance on Unix
            ruler_column: 0,                      // No ruler by default (0 = disabled)
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

        Self::load_from_path(&path)
    }

    fn load_from_path(path: &Path) -> Self {
        let content = match fs::read(path) {
            Ok(content) => content,
            Err(_) => return Self::default(),
        };
        match std::str::from_utf8(&content).ok().and_then(Self::parse) {
            Some(config) => config,
            None => {
                let config = Self::default();
                // save_to_path first preserves the corrupt bytes. If that cannot
                // be done, leave the user's original file untouched.
                let _ = config.save_to_path(path);
                config
            }
        }
    }

    fn preserve_corrupt_config(path: &Path, content: &[u8]) -> std::io::Result<()> {
        use std::io::Write;

        // Never overwrite an earlier recovery. create_new also protects against
        // another OGEdit process choosing the same backup name concurrently.
        for index in 0..1000 {
            let suffix =
                if index == 0 { ".backup".to_string() } else { format!(".backup.{index}") };
            let mut backup_name = path.as_os_str().to_os_string();
            backup_name.push(suffix);
            let backup = PathBuf::from(backup_name);
            let mut file = match fs::OpenOptions::new().write(true).create_new(true).open(&backup) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            };
            let result = file.write_all(content).and_then(|()| file.sync_all());
            drop(file);
            if result.is_err() {
                let _ = fs::remove_file(&backup);
            }
            return result;
        }
        Err(std::io::Error::other("Too many configuration recovery backups"))
    }

    /// Save configuration to ~/.ogedit/state.json.
    pub fn save(&self) -> apperr::Result<()> {
        let Some(path) = Self::config_path() else {
            return Ok(());
        };
        self.save_to_path(&path).map_err(apperr::Error::from)
    }

    fn save_to_path(&self, path: &Path) -> std::io::Result<()> {
        // Protect damaged settings even when a previous startup recovery failed.
        match fs::read(path) {
            Ok(content) if std::str::from_utf8(&content).ok().and_then(Self::parse).is_none() => {
                Self::preserve_corrupt_config(path, &content)?;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = self.to_json();
        atomicwrites::AtomicFile::new(path, atomicwrites::OverwriteBehavior::AllowOverwrite)
            .write(|file| {
                use std::io::Write;
                file.write_all(json.as_bytes())?;
                file.sync_all()
            })
            .map_err(|err| std::io::Error::other(err.to_string()))
    }

    /// Get the path to the configuration file (~/.ogedit/state.json)
    pub fn data_dir() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("OGEDIT_DATA_DIR")
            && !path.is_empty()
        {
            return Some(PathBuf::from(path));
        }
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
        Some(PathBuf::from(home).join(".ogedit"))
    }

    fn config_path() -> Option<PathBuf> {
        Some(Self::data_dir()?.join("state.json"))
    }

    /// Parse JSON with full-line // comments, preserving the documented format.
    fn parse(json: &str) -> Option<Self> {
        // Use a complete JSON parser, so nested fields, strings, malformed tokens,
        // and Unicode escapes cannot be mistaken for top-level settings.
        let json = json.trim_start_matches('\u{feff}');
        let json = json
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let value: serde_json::Value = serde_json::from_str(&json).ok()?;
        let object = value.as_object()?;
        let mut config = Self::default();
        macro_rules! boolean {
            ($field:ident) => {
                if let Some(value) = object.get(stringify!($field)) {
                    config.$field = value.as_bool()?;
                }
            };
        }
        boolean!(word_wrap);
        boolean!(indent_with_tabs);
        boolean!(newline_crlf);
        boolean!(line_numbers);
        boolean!(line_highlight);
        boolean!(insert_final_newline);
        if let Some(value) = object.get("tab_size") {
            config.tab_size = u8::try_from(value.as_u64()?).ok()?;
            if !(1..=8).contains(&config.tab_size) {
                return None;
            }
        }
        if let Some(value) = object.get("ruler_column") {
            config.ruler_column = u8::try_from(value.as_u64()?).ok()?;
        }
        if let Some(value) = object.get("project_folders") {
            for (key, value) in value.as_object()? {
                config.project_folders.insert(key.clone(), value.as_str()?.to_owned());
            }
        }
        if let Some(value) = object.get("recent_files") {
            for entry in value.as_array()? {
                let path = PathBuf::from(entry.get("path")?.as_str()?);
                let opened_at = entry.get("opened_at")?.as_u64()?;
                if !path.as_os_str().is_empty() {
                    config.recent_files.push(RecentFile { path, opened_at });
                }
            }
            config.normalize_recent_files();
        }
        // Invalid hotkey fields are explicitly documented to fall back individually.
        if let Some(hotkeys) = object.get("hotkeys").and_then(serde_json::Value::as_object) {
            macro_rules! hotkey {
                ($field:ident) => {
                    if let Some(key) = hotkeys
                        .get(stringify!($field))
                        .and_then(serde_json::Value::as_str)
                        .and_then(Hotkeys::parse_hotkey)
                    {
                        config.hotkeys.$field = key;
                    }
                };
            }
            hotkey!(file_new);
            hotkey!(file_open);
            hotkey!(file_save);
            hotkey!(file_save_as);
            hotkey!(file_reload);
            hotkey!(file_close);
            hotkey!(file_exit);
            hotkey!(edit_undo);
            hotkey!(edit_redo);
            hotkey!(edit_cut);
            hotkey!(edit_copy);
            hotkey!(edit_paste);
            hotkey!(edit_duplicate_line);
            hotkey!(edit_find);
            hotkey!(edit_replace);
            hotkey!(edit_find_next);
            hotkey!(edit_select_all);
            hotkey!(view_go_to_file);
            hotkey!(view_go_to_line);
            hotkey!(view_word_wrap);
        }
        Some(config)
    }

    fn normalize_recent_files(&mut self) {
        // Stable sorting retains the most-recent-first order for equal timestamps.
        self.recent_files.sort_by(|a, b| b.opened_at.cmp(&a.opened_at));
        let mut seen = HashSet::new();
        let mut recent = Vec::with_capacity(self.recent_files.len().min(100));
        for mut file in std::mem::take(&mut self.recent_files) {
            file.path = Self::recent_file_path(&file.path);
            if seen.insert(file.path.clone()) {
                recent.push(file);
                if recent.len() == 100 {
                    break;
                }
            }
        }
        self.recent_files = recent;
    }

    fn recent_file_path(path: &Path) -> PathBuf {
        // Opening through the picker and palette may yield different spellings
        // (notably C:\file and \\?\C:\file on Windows). Existing paths share one
        // canonical identity; unavailable paths retain their original spelling
        // so a disconnected drive or symlink can be resolved when it returns.
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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
                "  // Letters, digits and Space require Ctrl or Alt (Shift alone produces text).\n",
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
        json.push_str(&format!(
            "    \"file_new\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.file_new)
        ));
        json.push_str(&format!(
            "    \"file_open\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.file_open)
        ));
        json.push_str(&format!(
            "    \"file_save\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.file_save)
        ));
        json.push_str(&format!(
            "    \"file_save_as\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.file_save_as)
        ));
        json.push_str(&format!(
            "    \"file_reload\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.file_reload)
        ));
        json.push_str(&format!(
            "    \"file_close\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.file_close)
        ));
        json.push_str(&format!(
            "    \"file_exit\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.file_exit)
        ));

        // Edit operations
        json.push_str("\n    // Edit operations\n");
        json.push_str(&format!(
            "    \"edit_undo\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_undo)
        ));
        json.push_str(&format!(
            "    \"edit_redo\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_redo)
        ));
        json.push_str(&format!(
            "    \"edit_cut\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_cut)
        ));
        json.push_str(&format!(
            "    \"edit_copy\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_copy)
        ));
        json.push_str(&format!(
            "    \"edit_paste\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_paste)
        ));
        json.push_str(&format!(
            "    \"edit_duplicate_line\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_duplicate_line)
        ));
        json.push_str(&format!(
            "    \"edit_find\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_find)
        ));
        json.push_str(&format!(
            "    \"edit_replace\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_replace)
        ));
        json.push_str(&format!(
            "    \"edit_find_next\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_find_next)
        ));
        json.push_str(&format!(
            "    \"edit_select_all\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.edit_select_all)
        ));

        // View operations
        json.push_str("\n    // View operations\n");
        json.push_str(&format!(
            "    \"view_go_to_file\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.view_go_to_file)
        ));
        json.push_str(&format!(
            "    \"view_go_to_line\": \"{}\",\n",
            Hotkeys::hotkey_to_string(hotkeys.view_go_to_line)
        ));
        json.push_str(&format!(
            "    \"view_word_wrap\": \"{}\"\n",
            Hotkeys::hotkey_to_string(hotkeys.view_word_wrap)
        ));

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
        // String serialization is infallible; this includes every JSON control
        // character and keeps user paths round-trippable on Unix as well.
        let quoted = serde_json::to_string(s).expect("serializing a string cannot fail");
        quoted[1..quoted.len() - 1].to_owned()
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
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);

        self.record_recent_file(path, now);
        let _ = self.save();
    }

    fn record_recent_file(&mut self, path: &Path, now: u64) {
        self.normalize_recent_files();
        let path = Self::recent_file_path(path);
        // Reopening within the same second must still move the file to the top.
        // Keep order stable after reload even if the system clock moves backwards.
        let opened_at = now.max(self.recent_files.first().map_or(0, |file| file.opened_at));
        self.recent_files.retain(|file| file.path != path);
        self.recent_files.insert(0, RecentFile { path, opened_at });
        self.recent_files.truncate(100);
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
        original
            .project_folders
            .insert("/home/user/project".to_string(), "/home/user/project/src".to_string());
        original
            .project_folders
            .insert("C:\\Users\\Test".to_string(), "C:\\Users\\Test\\Documents".to_string());

        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed.project_folders, original.project_folders);
    }

    #[test]
    fn test_project_folders_escaping() {
        // Test that special characters in paths are properly escaped and parsed
        let mut original = Config::default();
        original
            .project_folders
            .insert("path\"with\"quotes".to_string(), "value\\with\\backslashes".to_string());

        let json = original.to_json();
        let parsed = Config::parse(&json).unwrap();
        assert_eq!(parsed.project_folders, original.project_folders);
    }

    #[test]
    fn test_recent_files_roundtrip() {
        // Test that recent_files can be serialized and deserialized
        let mut original = Config::default();
        original
            .recent_files
            .push(RecentFile { path: PathBuf::from("/home/user/file.txt"), opened_at: 1732900000 });
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
        // Printable keys without Ctrl/Alt arrive as text and cannot bind commands.
        assert_eq!(Hotkeys::parse_hotkey("S"), None);
        assert_eq!(Hotkeys::parse_hotkey("A"), None);
        assert_eq!(Hotkeys::parse_hotkey("Z"), None);

        // Digits
        assert_eq!(Hotkeys::parse_hotkey("0"), None);
        assert_eq!(Hotkeys::parse_hotkey("9"), None);
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
        assert!(Hotkeys::parse_hotkey("Ctrl+Space").is_some());
        assert!(Hotkeys::parse_hotkey("Space").is_none());
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

        // Shift plus a printable key is text, not a reliable terminal shortcut.
        assert!(Hotkeys::parse_hotkey("Shift+A").is_none());

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
        let test_cases =
            ["Ctrl+S", "Ctrl+Shift+S", "Alt+Z", "F5", "F3", "Ctrl+N", "Ctrl+W", "Ctrl+P", "Ctrl+G"];

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

        config
            .recent_files
            .push(RecentFile { path: PathBuf::from("/test/file.txt"), opened_at: 1000 });

        assert_eq!(config.recent_files.len(), 1);
        assert_eq!(config.recent_files[0].path, PathBuf::from("/test/file.txt"));
    }

    #[test]
    fn test_recent_files_sorting() {
        let mut config = Config::default();

        config.recent_files.push(RecentFile { path: PathBuf::from("/old.txt"), opened_at: 1000 });
        config.recent_files.push(RecentFile { path: PathBuf::from("/new.txt"), opened_at: 2000 });
        config
            .recent_files
            .push(RecentFile { path: PathBuf::from("/middle.txt"), opened_at: 1500 });

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

    #[test]
    fn parser_rejects_malformed_tokens_and_nested_setting_confusion() {
        for json in [
            r#"{"word_wrap":truegarbage}"#,
            r#"{"tab_size":4.5}"#,
            r#"{"tab_size":4oops}"#,
            r#"{"word_wrap":true "tab_size":4}"#,
            r#"{"word_wrap":true,}"#,
            r#"{"project_folders":{"project":123}}"#,
            r#"{"recent_files":[{"path":"a","opened_at":-1}]}"#,
        ] {
            assert!(Config::parse(json).is_none(), "accepted {json}");
        }
        let config =
            Config::parse(r#"{"future":{"word_wrap":true,"tab_size":8},"word_wrap":false}"#)
                .unwrap();
        assert!(!config.word_wrap);
        assert_eq!(config.tab_size, 4);
    }

    #[test]
    fn parser_supports_unicode_escapes_and_all_string_controls() {
        let config = Config::parse(r#"{"project_folders":{"\u0061":"\uD83D\uDE80"}}"#).unwrap();
        assert_eq!(config.get_project_folder("a"), Some("🚀"));
        let mut config = Config::default();
        config.project_folders.insert("control\0\x08\x0c\x1f".into(), "é🚀\t\n".into());
        assert_eq!(Config::parse(&config.to_json()), Some(config));
        assert!(Config::parse("\u{feff}{\"word_wrap\":true}").unwrap().word_wrap);
    }

    #[test]
    fn hotkeys_trim_components_and_fall_back_for_unusable_text_bindings() {
        assert_eq!(
            Hotkeys::parse_hotkey(" Ctrl + Alt + Shift + s "),
            Some(kbmod::CTRL_ALT_SHIFT | vk::S)
        );
        assert!(Hotkeys::parse_hotkey("Ctrl+Ctrl+S").is_none());
        let config = Config::parse(
            r#"{"hotkeys":{"file_save":"Shift+S","file_open":12,"file_close":"Ctrl + Alt + W"}}"#,
        )
        .unwrap();
        assert_eq!(config.hotkeys.file_save, Hotkeys::default().file_save);
        assert_eq!(config.hotkeys.file_open, Hotkeys::default().file_open);
        assert_eq!(config.hotkeys.file_close, kbmod::CTRL_ALT | vk::W);
    }

    #[test]
    fn recent_files_reopen_moves_to_front_even_with_equal_or_backwards_clock() {
        let mut config = Config::default();
        config.record_recent_file(Path::new("first.txt"), 100);
        config.record_recent_file(Path::new("second.txt"), 100);
        assert_eq!(config.recent_files[0].path, Path::new("second.txt"));
        config.record_recent_file(Path::new("first.txt"), 99);
        assert_eq!(config.recent_files.len(), 2);
        assert_eq!(config.recent_files[0].path, Path::new("first.txt"));
        assert_eq!(Config::parse(&config.to_json()).unwrap().recent_files, config.recent_files);
        for index in 0..110 {
            config.record_recent_file(Path::new(&format!("file-{index}.txt")), 100);
        }
        assert_eq!(config.recent_files.len(), 100);
        assert_eq!(config.recent_files[0].path, Path::new("file-109.txt"));
    }

    #[test]
    fn recent_files_loaded_are_sorted_deduplicated_and_bounded() {
        let mut entries = vec![serde_json::json!({"path":"duplicate", "opened_at":999})];
        for index in 0..110 {
            entries.push(serde_json::json!({"path":format!("file-{index}"), "opened_at":index}));
        }
        entries.push(serde_json::json!({"path":"duplicate", "opened_at":0}));
        let config =
            Config::parse(&serde_json::json!({"recent_files":entries}).to_string()).unwrap();
        assert_eq!(config.recent_files.len(), 100);
        assert_eq!(config.recent_files[0].path, Path::new("duplicate"));
        assert_eq!(config.recent_files[1].path, Path::new("file-109"));
        assert_eq!(
            config.recent_files.iter().filter(|entry| entry.path == Path::new("duplicate")).count(),
            1
        );
    }

    #[test]
    fn recent_file_aliases_share_one_entry_and_reopen_at_the_front() {
        let folder = std::env::temp_dir().join(format!(
            "ogedit-config-recent-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&folder).unwrap();
        let raw = folder.join("unicode-🚀.txt");
        let dotted = folder.join(".").join("unicode-🚀.txt");
        let other = folder.join("other.txt");
        let missing = folder.join("unavailable").join("..").join("retained.txt");
        fs::write(&raw, "fixture").unwrap();
        fs::write(&other, "second").unwrap();
        let canonical = fs::canonicalize(&raw).unwrap();
        let canonical_other = fs::canonicalize(&other).unwrap();

        let mut config = Config::default();
        config.record_recent_file(&raw, 10);
        config.record_recent_file(&other, 20);
        config.record_recent_file(&canonical, 30);
        config.record_recent_file(&dotted, 29);
        assert_eq!(config.recent_files.len(), 2);
        assert_eq!(config.recent_files[0], RecentFile { path: canonical.clone(), opened_at: 30 });
        assert_eq!(config.recent_files[1].path, canonical_other);

        // Old settings may already contain raw/verbatim/dot aliases. Keep the
        // newest alias, then preserve distinct files and unavailable paths.
        let json = serde_json::json!({ "recent_files": [
            {"path":raw, "opened_at":30},
            {"path":canonical, "opened_at":50},
            {"path":dotted, "opened_at":40},
            {"path":other, "opened_at":45},
            {"path":missing, "opened_at":35}
        ]});
        let mut loaded = Config::parse(&json.to_string()).unwrap();
        assert_eq!(loaded.recent_files.len(), 3);
        assert_eq!(loaded.recent_files[0], RecentFile { path: canonical.clone(), opened_at: 50 });
        assert_eq!(loaded.recent_files[1].path, canonical_other);
        assert_eq!(loaded.recent_files[2].path, missing);
        loaded.record_recent_file(&other, 60);
        assert_eq!(loaded.recent_files[0].path, canonical_other);
        loaded.record_recent_file(&raw, 60);
        assert_eq!(loaded.recent_files[0].path, canonical);
        assert_eq!(loaded.recent_files.len(), 3);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn corrupt_configuration_recovers_without_destroying_previous_backups() {
        let folder = std::env::temp_dir().join(format!(
            "ogedit-config-recovery-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("state.json");
        fs::write(&path, b"{broken-first}").unwrap();
        assert_eq!(Config::load_from_path(&path), Config::default());
        assert_eq!(fs::read(folder.join("state.json.backup")).unwrap(), b"{broken-first}");
        fs::write(&path, b"{broken-second}\xff").unwrap();
        assert_eq!(Config::load_from_path(&path), Config::default());
        assert_eq!(fs::read(folder.join("state.json.backup")).unwrap(), b"{broken-first}");
        assert_eq!(fs::read(folder.join("state.json.backup.1")).unwrap(), b"{broken-second}\xff");
        let config = Config { word_wrap: true, ..Config::default() };
        config.save_to_path(&path).unwrap();
        assert_eq!(Config::load_from_path(&path), config);
        fs::remove_dir_all(folder).unwrap();
    }
}
