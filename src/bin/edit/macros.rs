//! Persisted logical editing commands. No terminal bytes, file operations, or
//! clipboard dependencies are replayed; a paste stores its actual text.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomicwrites::{AllowOverwrite, AtomicFile};
use ogedit::buffer::{CursorMovement, TextBuffer};
use ogedit::clipboard::Clipboard;
use ogedit::helpers::{CoordType, Point};
use ogedit::input::{Input, InputKey, kbmod, vk};
use serde::{Deserialize, Serialize};

use crate::config::Config;

const MAX_COMMANDS: usize = 10_000;
const MAX_TEXT: usize = 1024 * 1024;
const MAX_FILE: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Grapheme,
    Word,
}

impl Granularity {
    fn movement(self) -> CursorMovement {
        match self {
            Self::Grapheme => CursorMovement::Grapheme,
            Self::Word => CursorMovement::Word,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Insert {
        text: String,
        raw: bool,
    },
    Paste {
        text: String,
        #[serde(default)]
        line_copy: bool,
        #[serde(default)]
        fragments: Option<Vec<String>>,
    },
    Move {
        granularity: Granularity,
        delta: CoordType,
        extend: bool,
    },
    Vertical {
        delta: CoordType,
        extend: bool,
    },
    Boundary {
        end: bool,
        document: bool,
        extend: bool,
    },
    Delete {
        granularity: Granularity,
        delta: CoordType,
    },
    Indent {
        direction: CoordType,
    },
    SelectAll,
    SelectLine,
    AddNext,
    AddCursor {
        delta: CoordType,
    },
    Rectangle {
        dx: CoordType,
        dy: CoordType,
    },
    ClearSelection,
    Duplicate,
    Cut,
    Copy,
    ToggleOvertype,
    MatchingBracket,
    Overtype {
        enabled: bool,
    },
}

impl Command {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Insert { text, .. } | Self::Paste { text, .. } if text.len() > MAX_TEXT => {
                Err("Macro text exceeds 1 MiB".into())
            }
            Self::Paste { fragments: Some(fragments), .. }
                if fragments.len() > MAX_COMMANDS
                    || fragments.iter().map(String::len).sum::<usize>() > MAX_TEXT =>
            {
                Err("Macro clipboard exceeds the limit".into())
            }
            Self::Move { delta, .. }
            | Self::Vertical { delta, .. }
            | Self::Delete { delta, .. }
                if delta.unsigned_abs() > 10_000 =>
            {
                Err("Macro movement exceeds 10,000 positions".into())
            }
            Self::AddCursor { delta } | Self::Indent { direction: delta }
                if ![-1, 1].contains(delta) =>
            {
                Err("Macro direction must be -1 or 1".into())
            }
            Self::Rectangle { dx, dy }
                if ![(-1, 0), (1, 0), (0, -1), (0, 1)].contains(&(*dx, *dy)) =>
            {
                Err("Invalid rectangle movement".into())
            }
            _ => Ok(()),
        }
    }

    fn apply(&self, buffer: &mut TextBuffer) {
        match self {
            Self::Insert { text, raw } => {
                if *raw {
                    buffer.write_raw(text.as_bytes());
                } else {
                    buffer.write_canon(text.as_bytes());
                }
            }
            Self::Paste { text, line_copy, fragments } => {
                let mut clipboard = Clipboard::default();
                clipboard.write(text.as_bytes().to_vec());
                if let Some(fragments) = fragments {
                    clipboard.write_fragments(
                        fragments.iter().map(|text| text.as_bytes().to_vec()).collect(),
                        if buffer.is_crlf() { b"\r\n" } else { b"\n" },
                    );
                }
                clipboard.write_was_line_copy(*line_copy);
                buffer.paste(&clipboard);
            }
            Self::Move { granularity, delta, extend } => {
                buffer.move_cursors_delta(granularity.movement(), *delta, *extend)
            }
            Self::Vertical { delta, extend } => buffer.move_cursors_vertical(*delta, *extend),
            Self::Boundary { end, document, extend } => {
                move_boundary(buffer, *end, *document, *extend)
            }
            Self::Delete { granularity, delta: -1 } => {
                buffer.backspace_with_auto_unindent(granularity.movement());
            }
            Self::Delete { granularity, delta } => buffer.delete(granularity.movement(), *delta),
            Self::Indent { direction } => buffer.indent_change(*direction),
            Self::SelectAll => buffer.select_all(),
            Self::SelectLine => buffer.select_line(),
            Self::AddNext => {
                buffer.add_next_occurrence();
            }
            Self::AddCursor { delta } => buffer.add_cursor_vertical(*delta),
            Self::Rectangle { dx, dy } => buffer.extend_rectangle(*dx, *dy),
            Self::ClearSelection => {
                buffer.clear_selection();
            }
            Self::Duplicate => buffer.duplicate_line_or_selection(),
            Self::Cut => buffer.cut(&mut Clipboard::default()),
            Self::Copy => {}
            Self::ToggleOvertype => buffer.set_overtype(!buffer.is_overtype()),
            Self::MatchingBracket => {
                buffer.jump_to_matching_bracket();
            }
            Self::Overtype { enabled } => buffer.set_overtype(*enabled),
        }
    }

    /// Capture before dispatch, then record only if the editor consumed the input.
    /// Unsupported input returns None so the UI can stop recording explicitly.
    pub fn from_input(input: &Input<'_>, clipboard: &[u8]) -> Option<Self> {
        match input {
            Input::Text(text) if text.len() <= MAX_TEXT => {
                Some(Self::Insert { text: (*text).to_owned(), raw: false })
            }
            Input::Paste(bytes) if bytes.len() <= MAX_TEXT => Some(Self::Paste {
                text: std::str::from_utf8(bytes).ok()?.to_owned(),
                line_copy: false,
                fragments: None,
            }),
            Input::Keyboard(key)
                if *key == (kbmod::CTRL | vk::V) || *key == (kbmod::SHIFT | vk::INSERT) =>
            {
                if clipboard.len() <= MAX_TEXT {
                    Some(Self::Paste {
                        text: std::str::from_utf8(clipboard).ok()?.to_owned(),
                        line_copy: false,
                        fragments: None,
                    })
                } else {
                    None
                }
            }
            Input::Keyboard(key) => command_from_key(*key),
            _ => None,
        }
    }

    pub fn from_input_with_clipboard(input: &Input<'_>, clipboard: &Clipboard) -> Option<Self> {
        let mut command = Self::from_input(input, clipboard.read())?;
        if matches!(input, Input::Keyboard(_))
            && let Self::Paste { line_copy, fragments, .. } = &mut command
        {
            *line_copy = clipboard.is_line_copy();
            *fragments = match clipboard.fragments() {
                Some(items) => Some(
                    items
                        .iter()
                        .map(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
                        .collect::<Result<Vec<_>, _>>()
                        .ok()?,
                ),
                None => None,
            };
        }
        Some(command)
    }
}

fn move_boundary(buffer: &mut TextBuffer, end: bool, document: bool, extend: bool) {
    if !document && buffer.selection_count() > 1 {
        buffer.move_cursors_line_boundary(end, extend);
        return;
    }
    let before = buffer.cursor_logical_pos();
    let pos = if document {
        if end { Point::MAX } else { Point::default() }
    } else {
        Point { x: if end { CoordType::MAX } else { 0 }, y: buffer.cursor_visual_pos().y }
    };
    if extend {
        buffer.selection_update_visual(pos);
    } else {
        buffer.cursor_move_to_visual(pos);
    }
    if !document {
        if buffer.is_word_wrap_enabled() && before == buffer.cursor_logical_pos() {
            let pos = Point { x: if end { CoordType::MAX } else { 0 }, y: before.y };
            if extend {
                buffer.selection_update_logical(pos);
            } else {
                buffer.cursor_move_to_logical(pos);
            }
        }
        if !end && buffer.cursor_logical_pos().x == 0 {
            let indent = buffer.indent_end_logical_pos();
            if before > indent || before.x == 0 {
                if extend {
                    buffer.selection_update_logical(indent);
                } else {
                    buffer.cursor_move_to_logical(indent);
                }
            }
        }
    }
}

fn move_vertical(
    buffer: &mut TextBuffer,
    delta: CoordType,
    extend: bool,
    preferred: &mut Option<CoordType>,
) {
    if buffer.selection_count() > 1 {
        buffer.move_cursors_vertical(delta, extend);
        return;
    }
    let mut x = *preferred.get_or_insert(buffer.cursor_visual_pos().x);
    let mut y = buffer.cursor_visual_pos().y + delta;
    if !extend && let Some((beg, end)) = buffer.selection_range() {
        let edge = if delta < 0 { beg } else { end };
        x = edge.visual_pos.x;
        y = edge.visual_pos.y + delta;
        *preferred = Some(x);
    }
    if y < 0 {
        x = 0;
        *preferred = Some(x);
    }
    if y >= buffer.visual_line_count() {
        x = CoordType::MAX;
    }
    let pos = Point { x, y };
    if extend {
        buffer.selection_update_visual(pos);
    } else {
        buffer.cursor_move_to_visual(pos);
    }
    if x == CoordType::MAX {
        *preferred = Some(buffer.cursor_visual_pos().x);
    }
}

#[derive(Serialize, Deserialize)]
struct SavedMacros {
    version: u32,
    macros: BTreeMap<String, Vec<Command>>,
}

#[derive(Default)]
pub struct MacroStore {
    saved: BTreeMap<String, Vec<Command>>,
    recording: Option<Vec<Command>>,
    recording_bytes: usize,
    recording_error: Option<String>,
    path: Option<PathBuf>,
}

impl MacroStore {
    pub fn load() -> Result<Self, String> {
        let path = Config::data_dir()
            .ok_or("Cannot locate the OGEdit settings directory")?
            .join("macros.json");
        Self::load_from(&path)
    }

    fn load_from(path: &Path) -> Result<Self, String> {
        let mut store = Self { path: Some(path.to_owned()), ..Self::default() };
        match fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(store),
            Err(e) => return Err(format!("Cannot read macros: {e}")),
            Ok(meta) if meta.len() > MAX_FILE => return Err("Macros file exceeds 8 MiB".into()),
            Ok(_) => {}
        }
        let data = fs::read(path).map_err(|e| format!("Cannot read macros: {e}"))?;
        let saved: SavedMacros =
            serde_json::from_slice(&data).map_err(|e| format!("Invalid macros file: {e}"))?;
        if saved.version != 1 {
            return Err("Unsupported macros file version".into());
        }
        for (name, commands) in &saved.macros {
            validate_name(name)?;
            validate_commands(commands)?;
        }
        store.saved = saved.macros;
        Ok(store)
    }

    pub fn names(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.saved.keys().map(String::as_str)
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn start(&mut self) {
        self.recording = Some(Vec::new());
        self.recording_bytes = 0;
        self.recording_error = None;
    }

    pub fn cancel(&mut self) {
        self.recording = None;
        self.recording_error = None;
    }

    pub fn record(&mut self, command: Command) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        if self.recording_error.is_some() {
            return;
        }
        if let Err(error) = command.validate() {
            self.recording_error = Some(error);
            return;
        }
        if let Command::Insert { text, .. } | Command::Paste { text, .. } = &command {
            self.recording_bytes += text.len();
        }
        if recording.len() >= MAX_COMMANDS || self.recording_bytes > MAX_TEXT {
            self.recording_error =
                Some("Macro recording exceeded 10,000 commands or 1 MiB of text".into());
            return;
        }
        // Canonical text is intentionally not merged: tabs, overtype and automatic
        // indentation can depend on each input boundary.
        recording.push(command);
    }

    #[cfg(test)]
    pub fn record_text(&mut self, text: &str, raw: bool) {
        self.record(Command::Insert { text: text.to_owned(), raw });
    }

    pub fn stop_and_save(&mut self, name: &str) -> Result<(), String> {
        validate_name(name)?;
        if self.saved.contains_key(name.trim()) {
            return Err("A macro already has that name. Choose a new name.".into());
        }
        if let Some(error) = &self.recording_error {
            return Err(error.clone());
        }
        let commands = self.recording.as_ref().ok_or("No macro is being recorded")?;
        if commands.is_empty() {
            return Err("Record at least one editing command before saving".into());
        }
        validate_commands(commands)?;
        let mut saved = self.saved.clone();
        saved.insert(name.trim().to_owned(), commands.clone());
        let data = serde_json::to_vec_pretty(&SavedMacros { version: 1, macros: saved.clone() })
            .map_err(|e| e.to_string())?;
        if data.len() as u64 > MAX_FILE {
            return Err("Saved macros exceed 8 MiB".into());
        }
        let path = self
            .path
            .clone()
            .or_else(|| Config::data_dir().map(|p| p.join("macros.json")))
            .ok_or("Cannot locate the OGEdit settings directory")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        AtomicFile::new(&path, AllowOverwrite)
            .write(|file| {
                file.write_all(&data)?;
                file.sync_all()
            })
            .map_err(|e| format!("Cannot save macro: {e}"))?;
        self.saved = saved;
        self.recording = None;
        self.path = Some(path);
        Ok(())
    }

    pub fn replay(&self, name: &str, buffer: &mut TextBuffer) -> Result<(), String> {
        if self.is_recording() {
            return Err("Finish recording before playing a macro".into());
        }
        let commands = self.saved.get(name).ok_or("Macro not found")?;
        validate_commands(commands)?;
        buffer.take_edit_capacity_error();
        let overtype_before = buffer.is_overtype();
        buffer.begin_undo_group();
        let mut preferred_column = None;
        for command in commands {
            if let Command::Vertical { delta, extend } = command {
                move_vertical(buffer, *delta, *extend, &mut preferred_column);
            } else {
                command.apply(buffer);
                preferred_column = None;
            }
            if buffer.take_edit_capacity_error() {
                buffer.abort_undo_group();
                buffer.set_overtype(overtype_before);
                return Err(
                    "Macro exceeds the document capacity; its edits were rolled back".into()
                );
            }
        }
        buffer.end_undo_group();
        buffer.make_cursor_visible();
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
        Err("Use a macro name of 1–80 characters without control characters".into())
    } else {
        Ok(())
    }
}

fn validate_commands(commands: &[Command]) -> Result<(), String> {
    if commands.len() > MAX_COMMANDS {
        return Err("Macro exceeds 10,000 commands".into());
    }
    let mut bytes = 0usize;
    for command in commands {
        command.validate()?;
        if let Command::Insert { text, .. } | Command::Paste { text, .. } = command {
            bytes = bytes.saturating_add(text.len());
        }
    }
    if bytes > MAX_TEXT {
        return Err("Macro text exceeds 1 MiB".into());
    }
    Ok(())
}

pub fn command_from_key(input: InputKey) -> Option<Command> {
    if let Some(ch) = input.text_char() {
        return Some(Command::Insert { text: ch.to_string(), raw: false });
    }
    let key = input.key();
    let modifiers = input.modifiers();
    let extend = modifiers.contains(kbmod::SHIFT);
    let word_modifier = if cfg!(target_os = "macos") { kbmod::ALT } else { kbmod::CTRL };
    let granularity =
        if modifiers.contains(word_modifier) { Granularity::Word } else { Granularity::Grapheme };
    if modifiers == kbmod::ALT_SHIFT {
        return match key {
            vk::LEFT => Some(Command::Rectangle { dx: -1, dy: 0 }),
            vk::RIGHT => Some(Command::Rectangle { dx: 1, dy: 0 }),
            vk::UP => Some(Command::Rectangle { dx: 0, dy: -1 }),
            vk::DOWN => Some(Command::Rectangle { dx: 0, dy: 1 }),
            _ => None,
        };
    }
    Some(match (key, modifiers) {
        (vk::D, kbmod::CTRL_ALT) => Command::AddNext,
        (vk::CLOSE_BRACKET, kbmod::CTRL) => Command::MatchingBracket,
        (vk::UP, kbmod::CTRL_ALT) => Command::AddCursor { delta: -1 },
        (vk::DOWN, kbmod::CTRL_ALT) => Command::AddCursor { delta: 1 },
        (vk::A, kbmod::CTRL) => Command::SelectAll,
        (vk::L, kbmod::CTRL) => Command::SelectLine,
        (vk::D, kbmod::CTRL) => Command::Duplicate,
        (vk::X, kbmod::CTRL) | (vk::DELETE, kbmod::SHIFT) => Command::Cut,
        (vk::C, kbmod::CTRL) | (vk::INSERT, kbmod::CTRL) => Command::Copy,
        (vk::INSERT, kbmod::NONE) => Command::ToggleOvertype,
        (vk::ESCAPE, kbmod::NONE) => Command::ClearSelection,
        (vk::TAB, kbmod::NONE) => Command::Indent { direction: 1 },
        (vk::TAB, kbmod::SHIFT) => Command::Indent { direction: -1 },
        (vk::RETURN, kbmod::NONE) => Command::Insert { text: "\n".into(), raw: false },
        (vk::BACK, kbmod::NONE | kbmod::CTRL) => Command::Delete { granularity, delta: -1 },
        (vk::DELETE, kbmod::NONE | kbmod::CTRL) => Command::Delete { granularity, delta: 1 },
        (vk::LEFT, _) => Command::Move { granularity, delta: -1, extend },
        (vk::RIGHT, _) => Command::Move { granularity, delta: 1, extend },
        (vk::UP, kbmod::NONE | kbmod::SHIFT) => Command::Vertical { delta: -1, extend },
        (vk::DOWN, kbmod::NONE | kbmod::SHIFT) => Command::Vertical { delta: 1, extend },
        (vk::HOME, _) => {
            Command::Boundary { end: false, document: modifiers.contains(kbmod::CTRL), extend }
        }
        (vk::END, _) => {
            Command::Boundary { end: true, document: modifiers.contains(kbmod::CTRL), extend }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        let mut b = TextBuffer::new(true).unwrap();
        b.set_crlf(false);
        b.write_raw(text.as_bytes());
        b.cursor_move_to_offset(0);
        b
    }

    #[test]
    fn replay_is_one_undo_and_persists_logical_commands() {
        let _runtime = crate::test_runtime::lock();
        let root = std::env::temp_dir().join(format!(
            "ogedit-macro-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let path = root.join("macros.json");
        let mut store = MacroStore::load_from(&path).unwrap();
        store.start();
        store.record(Command::AddNext);
        store.record(Command::AddNext);
        store.record_text("updated", true);
        store.stop_and_save("rename").unwrap();
        let store = MacroStore::load_from(&path).unwrap();
        let mut b = buffer("word word");
        store.replay("rename", &mut b).unwrap();
        assert_eq!(b.extract_bytes(0..b.text_length()), b"updated updated");
        b.undo();
        assert_eq!(b.extract_bytes(0..b.text_length()), b"word word");
        b.redo();
        assert_eq!(b.extract_bytes(0..b.text_length()), b"updated updated");
        fs::remove_file(path).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn capacity_failure_rolls_back_prior_macro_edits() {
        let _runtime = crate::test_runtime::lock();
        let commands = vec![
            Command::Insert { text: "first".into(), raw: true },
            Command::Insert { text: "x".repeat(128 * 1024), raw: true },
        ];
        let store = MacroStore {
            saved: BTreeMap::from([("huge".into(), commands)]),
            ..MacroStore::default()
        };
        let mut b = buffer("original");
        assert!(store.replay("huge", &mut b).is_err());
        assert_eq!(b.extract_bytes(0..b.text_length()), b"original");
        assert_eq!(b.cursor_offset(), 0);
        b.redo();
        assert_eq!(b.extract_bytes(0..b.text_length()), b"original");
    }

    #[test]
    fn malformed_commands_and_recording_limits_are_rejected() {
        assert!(validate_commands(&[Command::Rectangle { dx: 2, dy: 0 }]).is_err());
        assert!(validate_name("\n").is_err());
        let mut store = MacroStore::default();
        store.start();
        store.record_text(&"x".repeat(MAX_TEXT + 1), true);
        assert!(store.stop_and_save("oversized").is_err());
        assert!(store.is_recording());
    }

    #[test]
    fn canonical_macro_keeps_previous_typing_outside_undo_group() {
        let _runtime = crate::test_runtime::lock();
        let commands = vec![Command::Insert { text: "after".into(), raw: false }];
        let store = MacroStore {
            saved: BTreeMap::from([("typing".into(), commands)]),
            ..MacroStore::default()
        };
        let mut b = buffer("");
        b.write_canon(b"before");
        store.replay("typing", &mut b).unwrap();
        b.undo();
        assert_eq!(b.extract_bytes(0..b.text_length()), b"before");
    }

    #[test]
    fn large_macro_group_keeps_complete_undo_and_rollback() {
        let _runtime = crate::test_runtime::lock();
        let commands = vec![Command::Insert { text: "x".into(), raw: true }; 1200];
        let mut store = MacroStore {
            saved: BTreeMap::from([("long".into(), commands)]),
            ..MacroStore::default()
        };
        let mut b = buffer("before");
        store.replay("long", &mut b).unwrap();
        b.undo();
        assert_eq!(b.extract_bytes(0..b.text_length()), b"before");
        store.saved.get_mut("long").unwrap().extend([
            Command::ToggleOvertype,
            Command::Insert { text: "x".repeat(128 * 1024), raw: true },
        ]);
        assert!(store.replay("long", &mut b).is_err());
        assert_eq!(b.extract_bytes(0..b.text_length()), b"before");
        assert!(!b.is_overtype());
        // The failed transaction also preserves the redo history from before it.
        b.redo();
        assert_eq!(b.text_length(), 1206);
    }

    #[test]
    fn replay_keeps_vertical_goal_across_short_lines() {
        let _runtime = crate::test_runtime::lock();
        let commands = vec![Command::Vertical { delta: 1, extend: false }; 2];
        let store = MacroStore {
            saved: BTreeMap::from([("down".into(), commands)]),
            ..MacroStore::default()
        };
        let mut b = buffer("1234567890\nabc\n1234567890");
        b.cursor_move_to_logical(Point { x: 8, y: 0 });
        store.replay("down", &mut b).unwrap();
        assert_eq!(b.cursor_logical_pos(), Point { x: 8, y: 2 });
    }

    #[test]
    fn replay_preserves_line_copy_clipboard_metadata() {
        let _runtime = crate::test_runtime::lock();
        let mut b = buffer("one\ntwo");
        b.cursor_move_to_offset(2);
        let mut clipboard = Clipboard::default();
        b.copy(&mut clipboard);
        let command =
            Command::from_input_with_clipboard(&Input::Keyboard(kbmod::CTRL | vk::V), &clipboard)
                .unwrap();
        let store = MacroStore {
            saved: BTreeMap::from([("paste".into(), vec![command])]),
            ..MacroStore::default()
        };
        store.replay("paste", &mut b).unwrap();
        assert_eq!(b.extract_bytes(0..b.text_length()), b"one\none\ntwo");
    }
}
