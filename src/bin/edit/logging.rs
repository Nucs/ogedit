// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Debug logging system for ogedit.
//!
//! Logs user actions to `~/.ogedit/logs/{sanitized_cwd}_{YYYYMMDD}_{pid}.log`
//! for debugging and tracing.
//!
//! Log file naming:
//! - `{sanitized_cwd}`: Working directory with path separators replaced by `--`
//!   Example: `C:\My\Folder` becomes `c--my--folder`
//! - `{YYYYMMDD}`: Date in compact format
//! - `{pid}`: Process ID to ensure uniqueness when multiple instances run
//!
//! Example: `c--users--john--project_20251127_12345.log`

use std::cell::RefCell;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use ogedit::helpers::Point;
use ogedit::input::InputKey;

/// Session info captured at startup (immutable after init)
struct SessionInfo {
    sanitized_cwd: String,
    pid: u32,
}

/// Global session info - set once at init()
static SESSION_INFO: OnceLock<SessionInfo> = OnceLock::new();

// Thread-local logger instance
thread_local! {
    static LOGGER: RefCell<Option<Logger>> = const { RefCell::new(None) };
}

/// The logger struct that handles file writing
struct Logger {
    file: File,
    current_date: String,
}

impl Logger {
    fn new() -> Option<Self> {
        let session = SESSION_INFO.get()?;
        let (date, _) = get_datetime();
        let path = get_log_path(&session.sanitized_cwd, &date, session.pid)?;

        // Ensure the logs directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok()?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;

        Some(Self {
            file,
            current_date: date,
        })
    }

    fn ensure_current_date(&mut self) -> bool {
        let (date, _) = get_datetime();
        if date != self.current_date {
            // Date changed, open a new file
            let session = match SESSION_INFO.get() {
                Some(s) => s,
                None => return false,
            };
            if let Some(path) = get_log_path(&session.sanitized_cwd, &date, session.pid) {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if let Ok(file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    self.file = file;
                    self.current_date = date;
                    return true;
                }
            }
            return false;
        }
        true
    }

    fn write_log(&mut self, message: &str) {
        if !self.ensure_current_date() {
            return;
        }

        let (_, time) = get_datetime();
        let _ = writeln!(self.file, "[{}] {}", time, message);
        let _ = self.file.flush();
    }
}

/// Sanitize a path for use in a filename.
/// Converts path separators to `--` and removes/replaces invalid filename chars.
/// Example: `C:\Users\John\Project` -> `c--users--john--project`
fn sanitize_path_for_filename(path: &str) -> String {
    let mut result = String::with_capacity(path.len());

    for ch in path.chars() {
        match ch {
            // Path separators become --
            '/' | '\\' => {
                if !result.ends_with('-') {
                    result.push_str("--");
                }
            }
            // Colon (drive letter) becomes nothing, just skip
            ':' => {}
            // Invalid filename chars become underscore
            '<' | '>' | '"' | '|' | '?' | '*' => result.push('_'),
            // Whitespace becomes underscore
            ' ' | '\t' => result.push('_'),
            // Everything else lowercase
            c => result.push(c.to_ascii_lowercase()),
        }
    }

    // Trim leading/trailing dashes
    let trimmed = result.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Get the path to the log file
/// Format: ~/.ogedit/logs/{sanitized_cwd}_{yyyymmdd}_{pid}.log
fn get_log_path(sanitized_cwd: &str, date: &str, pid: u32) -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")?
    } else {
        std::env::var_os("HOME")?
    };

    let mut path = PathBuf::from(home);
    path.push(".ogedit");
    path.push("logs");

    // Compact date format: YYYYMMDD instead of YYYY-MM-DD
    let compact_date = date.replace('-', "");
    path.push(format!("{}_{compact_date}_{pid}.log", sanitized_cwd));
    Some(path)
}

/// Get current date and time as (YYYY-MM-DD, HH:MM:SS.mmm)
fn get_datetime() -> (String, String) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = now.as_secs();
    let millis = now.subsec_millis();

    // Calculate date/time components (simplified, assuming UTC)
    let days = total_secs / 86400;
    let time_of_day = total_secs % 86400;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Calculate year/month/day from days since epoch (1970-01-01)
    let (year, month, day) = days_to_ymd(days as i64);

    let date = format!("{:04}-{:02}-{:02}", year, month, day);
    let time = format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, millis);

    (date, time)
}

/// Convert days since Unix epoch to (year, month, day)
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as i32, m, d)
}

/// Initialize the logger with session information.
/// Must be called once at application startup before any logging.
///
/// Also installs a panic hook to capture panics to the log.
/// Note: In release builds with `panic = "abort"`, the hook may not run.
pub fn init() {
    // Capture session info (cwd and pid) - only done once
    let _ = SESSION_INFO.get_or_init(|| {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        SessionInfo {
            sanitized_cwd: sanitize_path_for_filename(&cwd),
            pid: process::id(),
        }
    });

    // Initialize the thread-local logger
    LOGGER.with(|logger| {
        let mut logger = logger.borrow_mut();
        if logger.is_none() {
            *logger = Logger::new();
        }
    });

    // Install panic hook to capture panics to log
    // Note: With panic=abort in release builds, this may not run
    install_panic_hook();
}

/// Install a panic hook that logs panics before the program terminates.
/// This captures the panic message and location to the log file.
fn install_panic_hook() {
    use std::panic;

    let default_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // Extract panic message
        let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };

        // Extract location
        let location = if let Some(loc) = panic_info.location() {
            format!("{}:{}:{}", loc.file(), loc.line(), loc.column())
        } else {
            "unknown location".to_string()
        };

        // Log the panic
        let panic_msg = format!("PANIC at {}: {}", location, message);
        write_log(&format!("!!! {} !!!", panic_msg));
        write_log("=== Application crashed ===");

        // Ensure log is flushed
        LOGGER.with(|logger| {
            if let Some(ref mut l) = *logger.borrow_mut() {
                let _ = l.file.flush();
            }
        });

        // Call the default hook (prints to stderr)
        default_hook(panic_info);
    }));
}

/// Write a log message
fn write_log(message: &str) {
    LOGGER.with(|logger| {
        if let Some(ref mut l) = *logger.borrow_mut() {
            l.write_log(message);
        }
    });
}

/// Escape special characters in text for logging.
/// Note: \r is stripped (not escaped) to normalize Windows line endings.
fn escape_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        match ch {
            '\n' => result.push_str("\\n"),
            '\r' => {} // Strip carriage returns (normalize CRLF to LF)
            '\t' => result.push_str("\\t"),
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            c if c.is_control() => {
                result.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => result.push(c),
        }
    }
    result
}

/// Escape special characters in bytes for logging.
/// Note: \r is stripped (not escaped) to normalize Windows line endings.
fn escape_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => escape_text(s),
        Err(_) => {
            // Fall back to hex representation for invalid UTF-8
            let mut result = String::with_capacity(bytes.len() * 4);
            for &b in bytes {
                if b == b'\r' {
                    // Strip carriage returns
                } else if b.is_ascii_graphic() || b == b' ' {
                    result.push(b as char);
                } else {
                    result.push_str(&format!("\\x{:02x}", b));
                }
            }
            result
        }
    }
}

/// Content size threshold for switching from inline to diff format
const CONTENT_THRESHOLD: usize = 256;

/// Format content for logging based on size:
/// - If < 256 chars: return escaped one-liner
/// - If >= 256 chars: return diff-style output with line numbers
fn format_content(text: &str) -> String {
    if text.len() < CONTENT_THRESHOLD {
        // Short content: escaped one-liner
        format!("\"{}\"", escape_text(text))
    } else {
        // Long content: diff-style with line numbers
        format_with_line_numbers(text)
    }
}

/// Format bytes content for logging based on size
fn format_content_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => format_content(s),
        Err(_) => {
            // Invalid UTF-8: show hex dump with line numbers if large
            if bytes.len() < CONTENT_THRESHOLD {
                format!("\"{}\"", escape_bytes(bytes))
            } else {
                format_hex_with_offset(bytes)
            }
        }
    }
}

/// Format text with line numbers in diff style
fn format_with_line_numbers(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let line_count = lines.len();
    let width = line_count.to_string().len().max(3); // At least 3 digits for alignment

    let mut result = format!("[CONTENT: {} bytes, {} lines]\n", text.len(), line_count);

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        let escaped_line = escape_text(line);
        // Truncate very long lines for readability
        let display_line = if escaped_line.len() > 200 {
            format!("{}...", &escaped_line[..200])
        } else {
            escaped_line
        };
        result.push_str(&format!("{:>width$}| {}\n", line_num, display_line));
    }

    result.trim_end().to_string()
}

/// Format binary data with hex dump and offsets
fn format_hex_with_offset(bytes: &[u8]) -> String {
    let mut result = format!("[BINARY: {} bytes]\n", bytes.len());

    for (i, chunk) in bytes.chunks(16).enumerate() {
        let offset = i * 16;
        result.push_str(&format!("{:08x}| ", offset));

        // Hex values
        for (j, &b) in chunk.iter().enumerate() {
            if j == 8 {
                result.push(' '); // Extra space at midpoint
            }
            result.push_str(&format!("{:02x} ", b));
        }

        // Padding for incomplete last line
        for j in chunk.len()..16 {
            if j == 8 {
                result.push(' ');
            }
            result.push_str("   ");
        }

        // ASCII representation
        result.push_str(" |");
        for &b in chunk {
            if b.is_ascii_graphic() || b == b' ' {
                result.push(b as char);
            } else {
                result.push('.');
            }
        }
        result.push_str("|\n");
    }

    result.trim_end().to_string()
}

/// Format a keyboard shortcut for logging
fn format_shortcut(key: InputKey) -> String {
    use ogedit::input::{kbmod, vk};

    // Match common shortcuts directly
    let shortcut_str = match key {
        k if k == kbmod::CTRL | vk::A => "Ctrl+A",
        k if k == kbmod::CTRL | vk::B => "Ctrl+B",
        k if k == kbmod::CTRL | vk::C => "Ctrl+C",
        k if k == kbmod::CTRL | vk::D => "Ctrl+D",
        k if k == kbmod::CTRL | vk::E => "Ctrl+E",
        k if k == kbmod::CTRL | vk::F => "Ctrl+F",
        k if k == kbmod::CTRL | vk::G => "Ctrl+G",
        k if k == kbmod::CTRL | vk::H => "Ctrl+H",
        k if k == kbmod::CTRL | vk::I => "Ctrl+I",
        k if k == kbmod::CTRL | vk::J => "Ctrl+J",
        k if k == kbmod::CTRL | vk::K => "Ctrl+K",
        k if k == kbmod::CTRL | vk::L => "Ctrl+L",
        k if k == kbmod::CTRL | vk::M => "Ctrl+M",
        k if k == kbmod::CTRL | vk::N => "Ctrl+N",
        k if k == kbmod::CTRL | vk::O => "Ctrl+O",
        k if k == kbmod::CTRL | vk::P => "Ctrl+P",
        k if k == kbmod::CTRL | vk::Q => "Ctrl+Q",
        k if k == kbmod::CTRL | vk::R => "Ctrl+R",
        k if k == kbmod::CTRL | vk::S => "Ctrl+S",
        k if k == kbmod::CTRL | vk::T => "Ctrl+T",
        k if k == kbmod::CTRL | vk::U => "Ctrl+U",
        k if k == kbmod::CTRL | vk::V => "Ctrl+V",
        k if k == kbmod::CTRL | vk::W => "Ctrl+W",
        k if k == kbmod::CTRL | vk::X => "Ctrl+X",
        k if k == kbmod::CTRL | vk::Y => "Ctrl+Y",
        k if k == kbmod::CTRL | vk::Z => "Ctrl+Z",
        k if k == kbmod::CTRL_SHIFT | vk::S => "Ctrl+Shift+S",
        k if k == kbmod::ALT | vk::Z => "Alt+Z",
        k if k == vk::F1 => "F1",
        k if k == vk::F2 => "F2",
        k if k == vk::F3 => "F3",
        k if k == vk::F4 => "F4",
        k if k == vk::F5 => "F5",
        k if k == vk::F6 => "F6",
        k if k == vk::F7 => "F7",
        k if k == vk::F8 => "F8",
        k if k == vk::F9 => "F9",
        k if k == vk::F10 => "F10",
        k if k == vk::F11 => "F11",
        k if k == vk::F12 => "F12",
        k if k == vk::RETURN => "Enter",
        k if k == vk::ESCAPE => "Escape",
        k if k == vk::BACK => "Backspace",
        k if k == vk::TAB => "Tab",
        k if k == vk::SPACE => "Space",
        k if k == vk::DELETE => "Delete",
        k if k == vk::INSERT => "Insert",
        k if k == vk::HOME => "Home",
        k if k == vk::END => "End",
        k if k == vk::PRIOR => "PageUp",
        k if k == vk::NEXT => "PageDown",
        k if k == vk::LEFT => "Left",
        k if k == vk::RIGHT => "Right",
        k if k == vk::UP => "Up",
        k if k == vk::DOWN => "Down",
        _ => {
            return "Unknown".to_string();
        }
    };

    shortcut_str.to_string()
}

// ============================================================================
// Public logging functions for various events
// ============================================================================

/// Log that the application started
pub fn log_app_start() {
    write_log("=== Application started ===");
    if let Some(session) = SESSION_INFO.get() {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        write_log(&format!("SESSION: cwd=\"{}\" pid={}", cwd, session.pid));
    }
}

/// Log that the application is exiting
pub fn log_app_exit() {
    write_log("=== Application exiting ===");
}

/// Log text input from the user
pub fn log_text_input(text: &str) {
    let formatted = format_content(text);
    write_log(&format!("TEXT_INPUT: {}", formatted));
}

/// Log text input from paste
pub fn log_paste(text: &[u8]) {
    let formatted = format_content_bytes(text);
    write_log(&format!("PASTE: {}", formatted));
}

/// Log a keyboard shortcut
pub fn log_shortcut(key: InputKey, action: &str) {
    let shortcut = format_shortcut(key);
    write_log(&format!("SHORTCUT: {} -> {}", shortcut, action));
}

/// Log cursor movement with offset information
/// - from/to: logical (column, line) positions
/// - from_offset/to_offset: byte offsets in buffer
/// - method: what caused the movement (e.g., "arrow_left", "arrow_up", "click", "navigation")
pub fn log_cursor_move(
    from: Point,
    to: Point,
    from_offset: usize,
    to_offset: usize,
    method: &str,
) {
    write_log(&format!(
        "CURSOR_MOVE: Ln {}, Col {} (offset {}) -> Ln {}, Col {} (offset {}) [{}]",
        from.y + 1,
        from.x + 1,
        from_offset,
        to.y + 1,
        to.x + 1,
        to_offset,
        method
    ));
}

/// Log selection with full details including offsets and content
/// - start/end: logical (column, line) positions
/// - start_offset/end_offset: byte offsets in buffer
/// - content: the selected text (will use adaptive formatting based on size)
pub fn log_selection(
    start: Point,
    end: Point,
    start_offset: usize,
    end_offset: usize,
    content: Option<&[u8]>,
) {
    let range_info = format!(
        "Ln {}, Col {} (offset {}) to Ln {}, Col {} (offset {})",
        start.y + 1,
        start.x + 1,
        start_offset,
        end.y + 1,
        end.x + 1,
        end_offset
    );

    if let Some(bytes) = content {
        let formatted = format_content_bytes(bytes);
        write_log(&format!("SELECTION: {} content={}", range_info, formatted));
    } else {
        write_log(&format!("SELECTION: {}", range_info));
    }
}

/// Log selection cleared
pub fn log_selection_clear() {
    write_log("SELECTION_CLEAR");
}

/// Log menu opened
pub fn log_menu_open(menu_name: &str) {
    write_log(&format!("MENU_OPEN: \"{}\"", menu_name));
}

/// Log menu item clicked
pub fn log_menu_click(menu_path: &str) {
    write_log(&format!("MENU_CLICK: \"{}\"", menu_path));
}

/// Log menu checkbox toggled
pub fn log_menu_checkbox(menu_path: &str, checked: bool) {
    let state = if checked { "checked" } else { "unchecked" };
    write_log(&format!("MENU_CHECKBOX: \"{}\" -> {}", menu_path, state));
}

/// Log file opened
pub fn log_file_open(path: &str) {
    write_log(&format!("FILE_OPEN: \"{}\"", escape_text(path)));
}

/// Log file saved
pub fn log_file_save(path: &str) {
    write_log(&format!("FILE_SAVE: \"{}\"", escape_text(path)));
}

/// Log new file created
pub fn log_file_new(name: &str) {
    write_log(&format!("FILE_NEW: \"{}\"", escape_text(name)));
}

/// Log file closed
pub fn log_file_close(name: &str, was_dirty: bool) {
    let dirty_str = if was_dirty { " (unsaved changes discarded)" } else { "" };
    write_log(&format!("FILE_CLOSE: \"{}\"{}",  escape_text(name), dirty_str));
}

/// Log search operation
pub fn log_search(needle: &str, found: bool) {
    let result = if found { "found" } else { "not found" };
    write_log(&format!("SEARCH: \"{}\" -> {}", escape_text(needle), result));
}

/// Log replace operation
pub fn log_replace(needle: &str, replacement: &str, count: usize) {
    write_log(&format!(
        "REPLACE: \"{}\" -> \"{}\" ({} occurrences)",
        escape_text(needle),
        escape_text(replacement),
        count
    ));
}

/// Log undo operation
pub fn log_undo() {
    write_log("UNDO");
}

/// Log redo operation
pub fn log_redo() {
    write_log("REDO");
}

/// Log goto line operation
pub fn log_goto(target: Point) {
    write_log(&format!("GOTO: line {}, column {}", target.y + 1, target.x + 1));
}

/// Log word wrap toggle
pub fn log_word_wrap_toggle(enabled: bool) {
    let state = if enabled { "enabled" } else { "disabled" };
    write_log(&format!("WORD_WRAP: {}", state));
}

/// Log encoding change
pub fn log_encoding_change(from: &str, to: &str) {
    write_log(&format!("ENCODING: \"{}\" -> \"{}\"", from, to));
}

/// Log delete operation
pub fn log_delete(deleted_text: &str, method: &str) {
    let formatted = format_content(deleted_text);
    write_log(&format!("DELETE: {} [{}]", formatted, method));
}

/// Log cut operation
pub fn log_cut(text: &[u8]) {
    let formatted = format_content_bytes(text);
    write_log(&format!("CUT: {}", formatted));
}

/// Log copy operation
pub fn log_copy(text: &[u8]) {
    let formatted = format_content_bytes(text);
    write_log(&format!("COPY: {}", formatted));
}

/// Log duplicate line operation
pub fn log_duplicate_line() {
    write_log("DUPLICATE_LINE");
}

/// Log select all operation
pub fn log_select_all() {
    write_log("SELECT_ALL");
}

/// Log mouse click with screen position and target area
pub fn log_mouse_click(pos: Point, button: &str, target: &str) {
    write_log(&format!("MOUSE_CLICK: ({},{}) [{}] -> {}", pos.x, pos.y, button, target));
}

/// Log mouse drag
pub fn log_mouse_drag(from: Point, to: Point) {
    write_log(&format!(
        "MOUSE_DRAG: ({},{}) -> ({},{})",
        from.x, from.y, to.x, to.y
    ));
}

/// Log dialog opened
pub fn log_dialog_open(dialog_name: &str) {
    write_log(&format!("DIALOG_OPEN: \"{}\"", dialog_name));
}

/// Log dialog closed
pub fn log_dialog_close(dialog_name: &str, result: &str) {
    write_log(&format!("DIALOG_CLOSE: \"{}\" -> {}", dialog_name, result));
}

/// Log generic action with custom message
pub fn log_action(action: &str) {
    write_log(&format!("ACTION: {}", action));
}

/// Log error
pub fn log_error(error: &str) {
    write_log(&format!("ERROR: {}", error));
}

/// Log content snapshot (periodic idle logging)
/// Uses the 256-byte threshold: short content is one-liner, long content gets line numbers
pub fn log_content_snapshot(content: &str, doc_name: &str) {
    let formatted = format_content(content);
    write_log(&format!("CONTENT_SNAPSHOT: doc=\"{}\" {}", escape_text(doc_name), formatted));
}
