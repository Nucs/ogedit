# Historical logging, watcher and reload review

Scope: Microsoft Edit parent `82cc84a` through OGEdit baseline `01cf53f`, plus a read-only check of today's watcher/reload integration. Code edits for this subreview are confined to `src/bin/edit/logging.rs`. Root owns document/state integration and browser QA. Existing modernization work is retained.

## Commit map

| Commit | Historical feature or repair | Review evidence |
| --- | --- | --- |
| `b8f09ef` (2025-11-27) | Debug logs by working directory/date/PID; application/input/action tracing and panic hook | Original logging was automatic, including entered/pasted/selected content. It was not a recovery mechanism. |
| `39d4f92` (2025-11-27) | Logging documentation | Documentation only. |
| `7686004` (2025-11-28) | Idle document snapshots and adaptive long-content formatting | Also repaired duplicate-line behavior. The long-line formatter introduced a UTF-8 slicing crash still present until this review. |
| `e3c588b` (2025-11-28) | File modification timestamps | Added baseline mtime and `has_file_changed_on_disk`; missing/unreadable files returned false. |
| `b881629` (2025-11-28) | Status-bar reload indicator/menu | Connected disk-change detection to the UI. |
| `3bea69e` (2025-11-29) | F5 reload and confirmations for local/external/both changes | Also fixed idle snapshots clearing dirty state by adding read-only `copy_content`. |
| `f7029af` (2025-11-29) | Native Windows/Linux/macOS/BSD watchers and fallback polling | Registered on open/save, removed on close. |
| `337772f` (2025-11-29) | Windows OVERLAPPED/event-list repair, Linux event deduplication, canonical paths, shutdown join | Removed duplicate status-bar polling. Its deliberate first-poll suppression could hide a real initial edit; today's replacement fixes this. |
| `9eb7304` (2025-11-29) | Watcher integration tests | Tests covered modification, deletion, unwatch, multiple files and fallback polling. |
| `09da7a6` (2025-11-29) | Cursor restoration after reload | Matches original line content, prefers closest duplicate, clamps when missing. |
| `3c28123`, `1b220e4` (2025-11-29) | Clippy cleanup | No separate user-facing persistence feature. |
| `7ee6888` (2025-11-29) | Active disk check on F5 | Reduces dependence on watcher timing, but historical checks still compared only mtime. |
| `067ffe7`, `7fc67cf` (2025-11-29) | macOS/Windows watcher-test reliability repairs | Test timing/platform fixes; no new editor interaction. |

## Confirmed findings repaired in logging.rs

1. **Ordinary Unicode paste/copy could crash even with logging disabled.** `format_with_line_numbers` used `&escaped_line[..200]`. A line containing 199 ASCII characters, then an emoji and enough trailing text to exceed 256 bytes splits that emoji at byte 200. Public logging functions formatted content before discovering no logger existed. Truncation now respects UTF-8 boundaries, and all public logging functions return before formatting when logging is disabled.
2. **Diagnostics could panic while already borrowed.** `write_log` and panic-hook flushing used unconditional `RefCell::borrow_mut`. A panic occurring while a logger borrow existed caused another panic in the panic hook. They now use best-effort `try_with` / `try_borrow_mut`, leaving the original panic/terminal-cleanup hook able to run.
3. **Binary traces ambiguously escaped quotes and literal backslashes.** Invalid-UTF-8 formatting escaped nonprintable bytes but emitted quotes/backslashes literally. It now escapes those delimiters as well.
4. **Custom shortcut logs said `Unknown`.** A fixed matcher omitted valid combinations and F13–F24. Labels now share the complete configuration formatter.

Five tests cover multibyte boundary truncation, binary escaping, custom shortcut labels, reentrant logger access and UTC leap/century date conversion. No desktop automation was used.

## Already fixed by today's modernization

- Logging is now explicitly enabled by `OGEDIT_LOG=1`, and state/idle-snapshot work is skipped by default. This opt-in change predated this review; the new early returns close the remaining default-mode formatting path. Explicit logging still includes editor content and file paths.
- Today's watcher tracks parent directories, preserving monitoring after atomic file replacement. It compares mtime, length and file identity, uses a timed metadata fallback, handles native rescan requests and no longer ignores a first genuine edit. Tests exercise immediate change, repeated atomic replacement, delete/recreate and one file being unwatched while another remains watched.
- Today's `DiskStatus` distinguishes deleted/unavailable files. Watch events can trigger a content-hash comparison when metadata alone looks unchanged. Changes for inactive documents are recorded, then checked when active.
- Saves are staged and synced, validate the disk baseline before replacement and mark buffers clean only after success. This replaces historical truncation of the destination before serialization.
- Reload loads into a temporary buffer and validates the disk before swapping it into the live document. A failed read leaves the user's original buffer intact. Cursor line-matching restoration remains.

## Integration finding reported to root

`state::reload_file_from_disk` claimed to retain the current encoding but called `doc.reread(None)`. `TextBuffer::read_file(None)` guesses from BOM and otherwise selects UTF-8. Consequently F5 could lose a manually chosen legacy encoding on a BOM-less file. Root was asked to preserve the active encoding for an ordinary reload while allowing explicit encoding-change actions to supply their chosen value, with a legacy-encoding regression test.

## Browser/manual QA recommendations

Run the real editor in the existing browser ConPTY harness with disposable fixture files and isolated `OGEDIT_DATA_DIR`:

- Paste a long line with an emoji crossing byte 200; copy/select it; confirm the editor stays responsive with default logging and with explicit logging enabled. Under opt-in logging, inspect the generated trace to confirm valid UTF-8 and custom shortcut names.
- Change a watched file externally immediately after opening; confirm the indicator appears. Repeat through atomic replacement twice, delete then recreate, and switch away/back to the affected document.
- Exercise F5 with external-only changes, local-only changes and both. Cancel must retain edits; confirm must load disk content. Deletion/read failure must show an error while preserving the buffer.
- Place the cursor on a unique line, insert lines externally before it and reload. Repeat with duplicate lines and with the original line removed; check nearest-match/clamping behavior.
- Open a BOM-less legacy-encoded fixture using an explicit encoding, modify it externally, reload and verify the encoding/content remain correct.
- After a successful save, confirm the changed-on-disk indicator clears. After failed save, confirm dirty state and recovery remain available.

Root runs Cargo and browser QA centrally. Useful filters are `logging::tests`, `watch::tests`, `documents::tests`, and `safe_save::tests`; final outcomes belong in the overall audit, not inferred from code inspection.

## Limits observed

Metadata-only polling cannot identify a same-length, same-mtime overwrite when the native backend misses or cannot deliver an event; the full content baseline at save still prevents silently overwriting that external content. Explicit debug logging has no retention limit and idle snapshots copy document content, so enabling it on enormous documents has a performance/storage cost. Log-file names embed the sanitized full working directory and may exceed filesystem component limits for very deep working directories; logging then fails quietly. Those are bounded limitations, not claims that all filesystems and all failure sequences have been exhaustively exercised.
