//! Resolve configurable commands before the editor's built-in key handling.

use ogedit::clipboard::Clipboard;
use ogedit::input::{Input, InputKey, kbmod, vk};

use crate::config::Hotkeys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    New,
    Open,
    Save,
    SaveAs,
    Reload,
    Close,
    Exit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Duplicate,
    Find,
    Replace,
    FindNext,
    SelectAll,
    Files,
    Goto,
    Wrap,
}

fn bindings(h: &Hotkeys) -> [(InputKey, Command); 20] {
    use Command::*;
    [
        (h.file_new, New),
        (h.file_open, Open),
        (h.file_save, Save),
        (h.file_save_as, SaveAs),
        (h.file_reload, Reload),
        (h.file_close, Close),
        (h.file_exit, Exit),
        (h.edit_undo, Undo),
        (h.edit_redo, Redo),
        (h.edit_cut, Cut),
        (h.edit_copy, Copy),
        (h.edit_paste, Paste),
        (h.edit_duplicate_line, Duplicate),
        (h.edit_find, Find),
        (h.edit_replace, Replace),
        (h.edit_find_next, FindNext),
        (h.edit_select_all, SelectAll),
        (h.view_go_to_file, Files),
        (h.view_go_to_line, Goto),
        (h.view_word_wrap, Wrap),
    ]
}

pub(crate) fn resolve(hotkeys: &Hotkeys, key: InputKey) -> Option<Command> {
    let current = bindings(hotkeys);
    let defaults = bindings(&Hotkeys::default());
    // An explicitly changed binding takes precedence over another action's default.
    current
        .iter()
        .zip(defaults)
        .find_map(|(&(binding, command), (default, _))| {
            (binding == key && binding != default).then_some(command)
        })
        .or_else(|| {
            current.iter().find_map(|&(binding, command)| (binding == key).then_some(command))
        })
}

impl Command {
    pub(crate) fn edits_buffer(self) -> bool {
        matches!(
            self,
            Self::Undo
                | Self::Redo
                | Self::Cut
                | Self::Copy
                | Self::Paste
                | Self::Duplicate
                | Self::SelectAll
        )
    }
}

pub(crate) fn macro_command(
    hotkeys: &Hotkeys,
    input: &Input<'_>,
    clipboard: &Clipboard,
) -> Option<crate::macros::Command> {
    use crate::macros::Command as Recorded;
    if let Input::Keyboard(key) = input {
        if let Some(command) = resolve(hotkeys, *key) {
            return match command {
                Command::Duplicate => Some(Recorded::Duplicate),
                Command::Cut => Some(Recorded::Cut),
                Command::Copy => Some(Recorded::Copy),
                Command::SelectAll => Some(Recorded::SelectAll),
                Command::Paste => Recorded::from_input_with_clipboard(
                    &Input::Keyboard(kbmod::CTRL | vk::V),
                    clipboard,
                ),
                _ => None, // Stop before file commands, history changes, or opening a panel.
            };
        }
        if *key == (kbmod::CTRL | vk::D) {
            return None;
        }
    }
    Recorded::from_input_with_clipboard(input, clipboard)
}

#[cfg(test)]
mod tests {
    use ogedit::input::{kbmod, vk};

    use super::*;

    #[test]
    fn all_declared_commands_resolve_and_custom_bindings_override_defaults() {
        let defaults = Hotkeys::default();
        for (key, command) in bindings(&defaults) {
            assert_eq!(resolve(&defaults, key), Some(command));
        }
        let hotkeys =
            Hotkeys { edit_undo: kbmod::CTRL | vk::S, file_save: kbmod::CTRL | vk::Z, ..defaults };
        assert_eq!(resolve(&hotkeys, kbmod::CTRL | vk::S), Some(Command::Undo));
        assert_eq!(resolve(&hotkeys, kbmod::CTRL | vk::Z), Some(Command::Save));
        let hotkeys = Hotkeys { edit_undo: kbmod::CTRL | vk::S, ..Hotkeys::default() };
        assert_eq!(resolve(&hotkeys, kbmod::CTRL | vk::S), Some(Command::Undo));
        assert_eq!(resolve(&hotkeys, kbmod::CTRL | vk::Z), None);
    }

    #[test]
    fn macro_recording_follows_the_resolved_command_not_the_original_key() {
        let hotkeys = Hotkeys {
            file_save: kbmod::CTRL | vk::V,
            edit_paste: vk::F8,
            edit_cut: vk::F7,
            ..Hotkeys::default()
        };
        let mut clipboard = Clipboard::default();
        clipboard.write(b"clipboard text".to_vec());
        assert!(
            macro_command(&hotkeys, &Input::Keyboard(kbmod::CTRL | vk::V), &clipboard).is_none()
        );
        assert!(matches!(
            macro_command(&hotkeys, &Input::Keyboard(vk::F7), &clipboard),
            Some(crate::macros::Command::Cut)
        ));
        assert!(
            matches!(macro_command(&hotkeys, &Input::Keyboard(vk::F8), &clipboard), Some(crate::macros::Command::Paste { text, .. }) if text == "clipboard text")
        );
    }
}
