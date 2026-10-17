//! Connect the asynchronous OS picker to the editor's normal open-file path.
use std::path::PathBuf;

use ogedit::tui::Context;

use crate::native_dialog::NativeFilePicker;
use crate::state::{
    DisplayablePathBuf, State, StateFilePicker, error_log_add, error_log_add_message,
};

fn initial_path(state: &State) -> PathBuf {
    if state.wants_file_picker == StateFilePicker::Open {
        let directory =
            state.file_picker_preview.input_directory(state.file_picker_pending_dir.as_path());
        let base = if directory.as_os_str().is_empty() {
            std::path::Path::new(&state.startup_cwd)
        } else {
            directory
        };
        return ogedit::path::join_user_input(base, &state.file_picker_pending_name);
    }
    state
        .documents
        .active()
        .and_then(|doc| doc.path.clone())
        .unwrap_or_else(|| PathBuf::from(&state.startup_cwd))
}

pub fn poll(ctx: &mut Context, state: &mut State) {
    if state.wants_native_browse {
        state.wants_native_browse = false;
        if state.native_file_picker.is_none() {
            match NativeFilePicker::start(initial_path(state)) {
                Ok(picker) => state.native_file_picker = Some(picker),
                Err(error) => error_log_add_message(ctx, state, error),
            }
        }
    }
    let Some(result) = state.native_file_picker.as_mut().and_then(NativeFilePicker::try_finish)
    else {
        return;
    };
    state.native_file_picker = None;
    match result {
        Ok(Some(path)) => match state.documents.add_file_path(&path, &state.config) {
            Ok(doc) => {
                if let Some(path) = &doc.path {
                    state.file_watcher.watch(path);
                    state.config.add_recent_file(path);
                    if let Some(parent) = path.parent() {
                        state.file_picker_pending_dir =
                            DisplayablePathBuf::from_path(parent.to_owned());
                        state.file_picker_pending_dir_revision =
                            state.file_picker_pending_dir_revision.wrapping_add(1);
                    }
                }
                state.wants_file_picker = StateFilePicker::None;
                state.file_picker_pending_name.clear();
                state.file_picker_preview.reset();
                state.file_picker_entries = None;
                state.file_picker_autocomplete.clear();
                state.file_picker_overwrite_warning = None;
            }
            Err(error) => error_log_add(ctx, state, error),
        },
        Ok(None) => {} // Cancel preserves the TUI picker's path and the active buffer.
        Err(error) => error_log_add_message(ctx, state, error),
    }
    ctx.needs_rerender();
}
