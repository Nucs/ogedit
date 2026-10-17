# Historical configuration and shortcuts review

Reviewed Microsoft Edit parent `82cc84a` through OGEdit baseline `01cf53f` (before the current uncommitted modernization). The current working tree retains today's features. `CLAUDE.md` was read as historical documentation; its version, binary size, test totals and watcher implementation no longer describe the current app.

## Commit map

| Commit | Historical behavior added | Code and review evidence |
| --- | --- | --- |
| `249501f` (2025-11-27) | Persistent Word Wrap, startup load, unknown-field tolerance and corrupt-file recovery | Added `config.rs`; threaded config into document creation and saved toggles from View. The original substring parser and backup handling are the source of the parsing/recovery findings below. |
| `26c591d` (2025-11-28) | Tabs/spaces, tab width, newline default, line numbers, line highlight, final newline, ruler, explanatory comments | Expanded config and document initialization; status-bar changes persist newline/indent preferences. Named files subsequently reset the ruler in `Document::update_file_mode`, overriding configured rulers. Reported to document-review owner for correction. |
| `fcc68d8` (2025-11-29) | Per-project Save As folder memory | `startup_cwd` is the project key. Successful Save As records the parent folder; startup uses the stored folder only if it still exists. Explicit command-line directory/file location takes precedence. |
| `cb15dae` (2025-11-29) | Recent files in Ctrl+P and Open | Added timestamped history and UI filters for missing/already-open documents. Original updates sorted by second-resolution timestamps, so two quick opens/reopens could display in the wrong order. Loading did not apply the 100-entry cap, deduplicate or sort. |
| `6a5c9ed` (2025-11-29) | Customizable file/edit/view shortcuts and displayed key labels | Config fields existed for all advertised actions, but several commands still dispatched through hardcoded textarea defaults. Global app handling occurred after textarea input, allowing default editing commands to swallow remapped shortcuts. Reported to root for dispatch integration. |
| `3c28123` (2025-11-29) | Clippy cleanup | Refactors only; no new end-user capability in configuration. |
| `3f95547` (2025-11-29) | Documentation and tests | Added parser/hotkey/history tests. Several history tests exercised direct Vec sorting/truncation rather than the update/load implementation; this audit adds behavior-level coverage. |

## Findings fixed in config.rs

- **Malformed settings silently accepted / nested fields misread.** Searching raw JSON for field names accepted `truegarbage`, truncated fractional numbers, omitted commas and top-level-looking fields inside unknown nested objects. It also failed to decode Unicode escapes. Replaced scanning with the already-installed `serde_json` parser, retaining full-line `//` comments and default values for missing settings. Known settings validate their types/ranges; unknown fields remain ignored. UTF-8 BOM is accepted.
- **Repeated corruption could destroy the preceding backup.** Recovery attempted an unchecked rename to a fixed filename, then saved defaults regardless of whether backup succeeded. Recovery now writes and syncs an exclusive backup; repeated failures use `.backup.1`, `.backup.2`, etc. Existing backups are never replaced. Failure to preserve the damaged bytes prevents overwriting the original. Invalid UTF-8 takes the same recovery route. Saves repeat this protection, so a later settings update cannot bypass an earlier failed recovery.
- **Incomplete JSON escaping.** Paths with NUL, backspace, form feed or other control characters could produce invalid JSON. String serialization now uses the complete JSON escaping implementation.
- **Recent-file ordering/cap defects.** Updates now move the file directly to the front, even during the same second. Timestamps remain monotonic for ordering after clock rollback. Loading sorts stably, removes duplicate paths and caps the collection at 100. Missing paths remain stored for temporarily unavailable drives; UI filtering decides visibility.
- **Unusable printable bindings and brittle whitespace.** Components now trim whitespace and accept modifiers case-insensitively; repeated/unknown modifiers are invalid. Letters, digits and Space require Ctrl or Alt, because terminals otherwise emit text. Invalid bindings fall back individually to their action's default. The generated configuration documents this requirement.

Today's existing atomic settings save and `OGEDIT_DATA_DIR` support were preserved. No new dependency was introduced.

## Integration findings delivered to root

- `edit_undo`, `edit_redo`, `edit_cut`, `edit_copy`, `edit_paste`, `edit_select_all` and `view_word_wrap` were configurable in labels but not dispatched as custom global bindings. `edit_find_next` is dispatched in `main.rs`; its presence was verified after a broader search.
- Dispatching after the textarea lets a default key such as Ctrl+Z undo instead of executing a remapped file action. Root owns the focus-aware shortcut integration and browser checks.
- `Document::update_file_mode` reset all non-COMMIT_EDITMSG rulers to zero after config application. Root/document reviewer owns the integration change and its tests.
- Historical logging stored literal typed/pasted/selected text and delayed document snapshots. Logging ownership remains with root; this is documented here to avoid implying that parsing hardening alone protects content privacy.

## Validation

New regression tests in `config.rs` cover malformed/nested JSON, Unicode and control-character round trips, BOM, per-action hotkey fallback and whitespace, same-second/backwards-clock history order, actual 100-item limit and loaded-history normalization, and repeated recovery including invalid UTF-8 while retaining all previous backups. File tests use unique temporary directories and private load/save helpers; they never change the user's settings or process environment.

Root reported the centralized test suite passed with the config changes (2026-10-17). The focused filter is `cargo test --locked --bin ogedit config::`. Root owns browser-only manual QA and final installation; final run evidence is recorded in the overall audit.

## Practical limits

Full-line `//` comments remain the documented configuration format; inline/block comments are not newly supported. Shortcut delivery still depends on terminal/browser-reserved keys. Configuration updates remain last-writer-wins across concurrently running editors; atomic replacement protects file integrity, not cross-process preference merging. No finite test suite can establish that every platform or future I/O failure is impossible.
