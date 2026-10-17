# OGEdit legacy commit map

Reviewed on 2026-10-17. This document maps the work committed **before today's modernization**. It is an audit inventory, not a claim that every historical commit is individually safe or has been executed.

## Reproducible boundaries

- Microsoft common ancestor: [`82cc84a`](https://github.com/microsoft/edit/commit/82cc84a610ded54d57f956285a8c4307c74bf364), Portuguese translations, 2025-10-31.
- Old OGEdit snapshot: [`01cf53f`](https://github.com/Nucs/ogedit/commit/01cf53f500831993d59d677fef92bc224b2e34bd), 2025-12-03, version 1.2.4.
- The actual fork delta is `git diff 82cc84a 01cf53f`: **46 commits, 40 paths, 7,887 inserted and 281 deleted lines**.
- Today's work was uncommitted when this audit began, so it is excluded by comparing explicit SHAs. Commit author dates alone are not used to decide this boundary.
- Latest Microsoft commit before 2026-10-17 00:00 Asia/Jerusalem (2026-10-16 21:00 UTC): [`cb6de0a`](https://github.com/microsoft/edit/commit/cb6de0a8966aecd01d19a3541dab1829d0e679ca), 2026-10-12. It has **122 upstream commits after the common ancestor**.
- `git diff 01cf53f cb6de0a` compares divergent snapshots: **198 paths, 20,658 insertions and 12,208 deletions** in the OGEdit-to-Microsoft direction. It must not be presented as OGEdit's contribution.

The machine-readable [commit inventory](legacy-commits.json) records full SHAs, author/commit timestamps, all changed paths and review notes. Both ranges are linear histories here; `git merge-base 01cf53f audit-microsoft/main` confirms the ancestor.

Full artifacts: [original fork patch](legacy-audit/fork-changes.patch), [Microsoft-at-cutoff to old-OGEdit snapshot patch](legacy-audit/upstream-at-cutoff-to-old-ogedit.patch), and [changed files](legacy-audit/changed-files.tsv). The second patch runs in the reverse direction from the illustrative OGEdit-to-Microsoft counts above.

## What belongs to which project

The terminal renderer, custom byte/gap buffer, SIMD scanning, Unicode handling, undo/redo, file open/save, in-file search/replace, encoding and newline conversion, word wrap, line numbers, status bar, fuzzy open-document switching, localization and platform backends already exist at the Microsoft ancestor. The fork adds persistence and behavior around many of these; it did not invent the underlying editor.

| Legacy fork capability | Commits introducing or materially changing it | Present implementation |
| --- | --- | --- |
| OGEdit name, CLI identity, About attribution | 82f2e42, 940f30b, cb15dae, 3f89875 | Retained. |
| Persistent wrap and editor defaults | 249501f, 26c591d | Retained and hardened in current settings code; original ad hoc parser is historical. |
| Duplicate current line / selected text | 940f30b, 1af1a9b, 7686004 | Retained; current editing engine extends transactions and multiple cursors. |
| Per-session debug logging and idle snapshots | b8f09ef, 39d4f92, 7686004, 3bea69e, 3f95547, 396c111 | Current logging controls and privacy behavior must be read from current code; historical logging captured text and snapshots by default. |
| Last Save As folder per launch directory | fcc68d8 | Retained. |
| Recent files in Open and Ctrl+P | cb15dae | Retained; Ctrl+P now also searches workspaces asynchronously. |
| External-change detection and confirmed F5 reload | e3c588b, b881629, 3bea69e, f7029af, 337772f, 7ee6888 | Retained behavior; today's notify-based watcher and safe persistence replace the custom native watcher. The temporary status reload popup did not survive the old history. |
| Cursor restoration after reload | 09da7a6 | Retained and reviewed with current buffer helpers. |
| Matching-selection highlights | 0ff7f96 | Retained; distinct from today's syntax/bracket highlighting. |
| Configurable shortcuts | 6a5c9ed, 337772f | Original file/edit/view remaps retained, with dispatch precedence reviewed. New workbench commands use fixed shortcuts and the command palette; their shortcuts are not configurable. |
| Gutter click/Shift+click/drag selection | 396c111 | Ported from Microsoft PR #622 by jenia90; Microsoft subsequently merged the feature as 0a6f8a3. Retained and reviewed in current TUI. |
| Packaging, optional EDITOR, version tags | a7ce6a9, 6344b1a, e0371c6, 9ae091e, 4a7beb0, e69f3a2, 01cf53f | Retained delivery intent; historical workflow and installer defects are repaired in the audit work. |

Today's editable workspace search, multi-cursor/macros, atomic saving/recovery/comparisons, structured CSV/JSON/log views, LSH syntax highlighting/bracket matching, command palette/workspace fuzzy lookup, native Browse and pasted-path navigation are **not legacy fork features**. LSH was vendored from later Microsoft work; it is not an original OGEdit implementation.

## Every fork commit, oldest first

Dates below are author dates in their original +02:00 timezone. Path lists come directly from each commit, including files that a later commit removed. Notes describe the actual diff; several historical subjects overstate or omit changes.

### 01. 82f2e42 — Rename project to ogedit and add comprehensive documentation

2025-11-27T07:55:59+02:00 · [commit](https://github.com/Nucs/ogedit/commit/82f2e4204df9d285d730c4b68dfec64309d5b458) · 12 files changed, 690 insertions(+), 59 deletions(-)

Renames Cargo package/binary, imports and CLI help/version to ogedit; adds developer documentation and dist ignore. The Cargo dependency lockfile is also refreshed.

Changed paths: `.gitignore` (M), `CLAUDE.md` (A), `Cargo.lock` (M), `Cargo.toml` (M), `src/bin/edit/documents.rs` (M), `src/bin/edit/draw_editor.rs` (M), `src/bin/edit/draw_filepicker.rs` (M), `src/bin/edit/draw_menubar.rs` (M), `src/bin/edit/draw_statusbar.rs` (M), `src/bin/edit/localization.rs` (M), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M).

### 02. 249501f — Add persistent Word Wrap configuration

2025-11-27T08:10:10+02:00 · [commit](https://github.com/Nucs/ogedit/commit/249501f2d8821a3c497e16bf23fcea763510cb30) · 6 files changed, 279 insertions(+), 34 deletions(-)

Adds ~/.ogedit/state.json, word-wrap persistence, startup defaults, JSON parsing/serialization and corrupt-config backup. Word wrap itself is inherited from Edit.

Changed paths: `src/bin/edit/config.rs` (A), `src/bin/edit/documents.rs` (M), `src/bin/edit/draw_filepicker.rs` (M), `src/bin/edit/draw_menubar.rs` (M), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M).

### 03. 940f30b — Update About dialog with OGEdit fork information

2025-11-27T08:25:33+02:00 · [commit](https://github.com/Nucs/ogedit/commit/940f30b50db1faa189fe664fbcdfc7e43a2d3ea7) · 3 files changed, 114 insertions(+), 1 deletion(-)

Updates About attribution AND introduces the duplicate-line/selection implementation, Edit > Duplicate and its English localization. The title understates the code change.

Changed paths: `i18n/edit.toml` (M), `src/bin/edit/draw_editor.rs` (M), `src/bin/edit/draw_menubar.rs` (M).

### 04. 1af1a9b — Add Ctrl+D duplicate line feature with comprehensive tests

2025-11-27T08:40:55+02:00 · [commit](https://github.com/Nucs/ogedit/commit/1af1a9b3d8da89ce629fd3c9d7be644189c0542f) · 1 file changed, 2 insertions(+)

Adds the two-line Ctrl+D shortcut dispatch. Despite the subject/body, this diff contains no tests; the implementation had landed in 940f30b.

Changed paths: `src/bin/edit/main.rs` (M).

### 05. b8f09ef — Add comprehensive debug logging system

2025-11-27T16:36:53+02:00 · [commit](https://github.com/Nucs/ogedit/commit/b8f09efc12241852e23a6c632b96d27f322aef81) · 7 files changed, 824 insertions(+), 24 deletions(-)

Adds per-session log files and instrumentation for input, search, menus, file operations, settings and cursor state. Logs include user text by default in the historical snapshot.

Changed paths: `src/bin/edit/draw_editor.rs` (M), `src/bin/edit/draw_filepicker.rs` (M), `src/bin/edit/draw_menubar.rs` (M), `src/bin/edit/draw_statusbar.rs` (M), `src/bin/edit/logging.rs` (A), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M).

### 06. 39d4f92 — Document debug logging system in CLAUDE.md

2025-11-27T23:03:41+02:00 · [commit](https://github.com/Nucs/ogedit/commit/39d4f92589f8dad71dca25189f94365e6cb34d33) · 3 files changed, 96 insertions(+), 1 deletion(-)

Adds a panic hook, startup/resize logging and documentation; this is not a documentation-only change.

Changed paths: `CLAUDE.md` (M), `src/bin/edit/logging.rs` (M), `src/bin/edit/main.rs` (M).

### 07. 7686004 — Fix Ctrl+D duplicate line crashes and improve behavior

2025-11-28T00:12:14+02:00 · [commit](https://github.com/Nucs/ogedit/commit/7686004cb5cd729fafa360a28887449e4c224599) · 6 files changed, 590 insertions(+), 170 deletions(-)

Moves duplication into TextBuffer, fixes line/selection and wrap handling, adds cursor byte offsets; also logs selected content and whole-document idle snapshots.

Changed paths: `CLAUDE.md` (M), `src/bin/edit/draw_editor.rs` (M), `src/bin/edit/logging.rs` (M), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M), `src/buffer/mod.rs` (M).

### 08. 26c591d — Expand configuration system with more settings and comments

2025-11-28T06:46:51+02:00 · [commit](https://github.com/Nucs/ogedit/commit/26c591d7ce9cab58a37a5135fc6071bbcc714cc1) · 4 files changed, 237 insertions(+), 53 deletions(-)

Adds persistent indentation, tab size, newline, line numbers, current-line highlight, final-newline and ruler defaults; supports full-line // comments and saves status-bar changes.

Changed paths: `CLAUDE.md` (M), `src/bin/edit/config.rs` (M), `src/bin/edit/documents.rs` (M), `src/bin/edit/draw_statusbar.rs` (M).

### 09. e3c588b — Add file modification tracking (Commit 1: timestamp detection)

2025-11-28T07:19:55+02:00 · [commit](https://github.com/Nucs/ogedit/commit/e3c588b1de7d30512acc311e28c590898222a369) · 1 file changed, 38 insertions(+)

Captures file mtimes on open/save/reload and exposes has_file_changed_on_disk; not yet a complete user-visible reload flow.

Changed paths: `src/bin/edit/documents.rs` (M).

### 10. b881629 — Add status bar reload indicator and menu (Commit 2: UI integration)

2025-11-28T10:43:51+02:00 · [commit](https://github.com/Nucs/ogedit/commit/b88162977fa2776aee837f6821c4e1d4fc0b6673) · 2 files changed, 63 insertions(+)

Adds a changed-on-disk status button and reload popup. That particular status UI is removed in cb15dae and replaced by File-menu reload in 3bea69e.

Changed paths: `src/bin/edit/draw_statusbar.rs` (M), `src/bin/edit/state.rs` (M).

### 11. 30960de — Update README for OGEdit fork and improve CI workflow

2025-11-28T21:30:41+02:00 · [commit](https://github.com/Nucs/ogedit/commit/30960dee92518c4bf5d8ada9f77b72fde3e772b1) · 3 files changed, 185 insertions(+), 74 deletions(-)

Rewrites README for the fork; adds macOS CI and optimized-build checks, cache fallback and removes formatting check; shortens the temporary reload status label.

Changed paths: `.github/workflows/ci.yml` (M), `README.md` (M), `src/bin/edit/draw_statusbar.rs` (M).

### 12. c42460a — Fix edit:: to use imported CoordType

2025-11-28T21:33:42+02:00 · [commit](https://github.com/Nucs/ogedit/commit/c42460ae0f6ec650ff3e8f20271ffea40efab607) · 1 file changed, 1 insertion(+), 1 deletion(-)

Fixes a remaining edit::helpers::CoordType reference in main after the crate rename.

Changed paths: `src/bin/edit/main.rs` (M).

### 13. c2786f8 — Fix edit:: to ogedit:: in benchmarks

2025-11-28T21:39:13+02:00 · [commit](https://github.com/Nucs/ogedit/commit/c2786f856d155688b9f4c18aeb4d241ef37f6f6c) · 1 file changed, 3 insertions(+), 3 deletions(-)

Renames benchmark crate imports from edit to ogedit.

Changed paths: `benches/lib.rs` (M).

### 14. d87a7f8 — Fix function pointer cast in Unix signal handler

2025-11-28T21:42:08+02:00 · [commit](https://github.com/Nucs/ogedit/commit/d87a7f8bb53e5f0d3cab6afa4e30fd7928b00983) · 1 file changed, 1 insertion(+), 1 deletion(-)

Uses an intermediate pointer cast for the Unix signal handler to satisfy the function-cast lint.

Changed paths: `src/sys/unix.rs` (M).

### 15. 5ddfbf9 — Use is_some_and instead of map_or(false, ...)

2025-11-28T21:44:33+02:00 · [commit](https://github.com/Nucs/ogedit/commit/5ddfbf9c0da4ffe5dd99e19ab4397aef232daca8) · 1 file changed, 1 insertion(+), 1 deletion(-)

Replaces map_or(false, ...) with is_some_and in status-bar change detection; behavior-preserving lint cleanup.

Changed paths: `src/bin/edit/draw_statusbar.rs` (M).

### 16. a7ce6a9 — Release v1.2.2

2025-11-28T22:00:31+02:00 · [commit](https://github.com/Nucs/ogedit/commit/a7ce6a9d659a5c74bbc2d205f809ccf3bc3f1fd2) · 4 files changed, 301 insertions(+), 41 deletions(-)

Bumps to 1.2.2, adds five-target release ZIP/checksum workflow, removes upstream WinGet workflow and documents fork release/versioning.

Changed paths: `.github/workflows/release.yml` (A), `.github/workflows/winget.yml` (D), `CLAUDE.md` (M), `Cargo.toml` (M).

### 17. 3f89875 — Add copyright notice for OGEdit fork

2025-11-29T09:47:57+02:00 · [commit](https://github.com/Nucs/ogedit/commit/3f89875f79da41371205b723228921df4ae71463) · 1 file changed, 1 insertion(+)

Adds the fork copyright line to LICENSE while retaining the Microsoft license.

Changed paths: `LICENSE` (M).

### 18. 6344b1a — Add MSI installer support for Windows releases

2025-11-29T09:48:35+02:00 · [commit](https://github.com/Nucs/ogedit/commit/6344b1ac749560711874e30774afcd5f21534b24) · 3 files changed, 267 insertions(+), 6 deletions(-)

Adds WiX MSI packages for Windows x64/ARM64, PATH integration, shortcuts, license and directory UI.

Changed paths: `.github/workflows/release.yml` (M), `wix/License.rtf` (A), `wix/main.wxs` (A).

### 19. fcc68d8 — Add per-project folder memory for Save As dialog

2025-11-29T09:49:18+02:00 · [commit](https://github.com/Nucs/ogedit/commit/fcc68d8f23d280dd22207779e77e6e62e6dad63f) · 6 files changed, 288 insertions(+), 12 deletions(-)

Persists last Save As directory keyed by launch working directory; chooses that directory on startup. Also introduces interim frame/time-based disk-check scheduling.

Changed paths: `CLAUDE.md` (M), `README.md` (M), `src/bin/edit/config.rs` (M), `src/bin/edit/draw_filepicker.rs` (M), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M).

### 20. cb15dae — Add recent files feature to Go to File dialog (Ctrl+P)

2025-11-29T09:49:43+02:00 · [commit](https://github.com/Nucs/ogedit/commit/cb15dae97219a1bf4654d0593e3b171d86f54959) · 9 files changed, 372 insertions(+), 65 deletions(-)

Adds up to 100 timestamped recent paths to Ctrl+P and Open, filters missing/open files, tracks startup/dialog opens, updates About maintainer. Also removes the earlier status reload popup and changes polling.

Changed paths: `CLAUDE.md` (M), `README.md` (M), `src/bin/edit/config.rs` (M), `src/bin/edit/documents.rs` (M), `src/bin/edit/draw_filepicker.rs` (M), `src/bin/edit/draw_menubar.rs` (M), `src/bin/edit/draw_statusbar.rs` (M), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M).

### 21. 3bea69e — Add reload dialog with confirmation for all scenarios

2025-11-29T09:54:48+02:00 · [commit](https://github.com/Nucs/ogedit/commit/3bea69ea193a7db2de0fa128a2210a87033fa986) · 6 files changed, 277 insertions(+), 9 deletions(-)

Adds File > Reload/F5 and dirty/external/both confirmation variants with localization, reload state and flags. Fixes idle snapshot logging marking documents clean by introducing read-only copy_content.

Changed paths: `i18n/edit.toml` (M), `src/bin/edit/draw_editor.rs` (M), `src/bin/edit/draw_menubar.rs` (M), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M), `src/buffer/mod.rs` (M).

### 22. f7029af — Add file watcher for detecting external modifications

2025-11-29T10:01:34+02:00 · [commit](https://github.com/Nucs/ogedit/commit/f7029afb89b28111b7ccf804263eba634ca61a73) · 6 files changed, 710 insertions(+), 2 deletions(-)

Adds background native watching (ReadDirectoryChangesW/inotify/kqueue) with timestamp fallback, register/unregister lifecycle and event handling in editor state.

Changed paths: `src/bin/edit/draw_editor.rs` (M), `src/bin/edit/draw_filepicker.rs` (M), `src/bin/edit/draw_statusbar.rs` (M), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M), `src/bin/edit/watch.rs` (A).

### 23. 337772f — Fix file watcher and remove duplicate polling

2025-11-29T12:03:47+02:00 · [commit](https://github.com/Nucs/ogedit/commit/337772f1f13d907e1a08f765820a487aec775618) · 5 files changed, 307 insertions(+), 298 deletions(-)

Reworks Windows overlapped watcher lifetime/events, canonicalizes paths, joins worker on drop, adjusts Unix debounce and fallback polling; removes duplicate frame polling and renders function-key menu shortcuts.

Changed paths: `src/bin/edit/draw_editor.rs` (M), `src/bin/edit/draw_statusbar.rs` (M), `src/bin/edit/state.rs` (M), `src/bin/edit/watch.rs` (M), `src/tui.rs` (M).

### 24. 9eb7304 — Add integration tests for file watcher

2025-11-29T14:58:48+02:00 · [commit](https://github.com/Nucs/ogedit/commit/9eb7304a8fd9c5ea1e896dd5a99d18eb67b2d7ef) · 2 files changed, 396 insertions(+), 1 deletion(-)

Adds native watcher integration tests and updates the lockfile. The OS-backed tests include timing/wait helpers.

Changed paths: `Cargo.lock` (M), `src/bin/edit/watch.rs` (M).

### 25. d120e7b — Fix crash when pasting over selection with word wrap enabled

2025-11-29T17:53:31+02:00 · [commit](https://github.com/Nucs/ogedit/commit/d120e7bf914c17eeb1aa5926d21b4ab4238ad23b) · 1 file changed, 7 insertions(+), 2 deletions(-)

Recomputes the safe cursor start after paste-over-selection with word wrap to avoid using visual positions from the old buffer.

Changed paths: `src/buffer/mod.rs` (M).

### 26. 0ff7f96 — Add selection auto-highlight feature

2025-11-29T18:10:43+02:00 · [commit](https://github.com/Nucs/ogedit/commit/0ff7f96097782386708dc3286e67c0a28673779c) · 1 file changed, 176 insertions(+)

Adds identical-selection occurrence highlighting, selection eligibility rules, generation/selection caching and a 1,000-match display cap.

Changed paths: `src/buffer/mod.rs` (M).

### 27. 09da7a6 — Restore cursor position after file reload

2025-11-29T18:41:03+02:00 · [commit](https://github.com/Nucs/ogedit/commit/09da7a6f6717ffb176859505c1ba0b068937972b) · 2 files changed, 180 insertions(+), 3 deletions(-)

Restores cursor on reload using saved-line matching/nearest location; adds line extraction/matching helpers and updates visual statistics before setting the cursor.

Changed paths: `src/bin/edit/documents.rs` (M), `src/buffer/mod.rs` (M).

### 28. 6a5c9ed — Add configurable keyboard shortcuts

2025-11-29T18:41:39+02:00 · [commit](https://github.com/Nucs/ogedit/commit/6a5c9ed5b756c3276f1a277b8d90d8996498204f) · 4 files changed, 410 insertions(+), 42 deletions(-)

Adds serialized configurable file/edit/view shortcuts, parser/defaults and menu/global dispatch; implements InputKey display formatting.

Changed paths: `src/bin/edit/config.rs` (M), `src/bin/edit/draw_menubar.rs` (M), `src/bin/edit/main.rs` (M), `src/input.rs` (M).

### 29. 99b33af — Update documentation for configurable hotkeys

2025-11-29T18:42:03+02:00 · [commit](https://github.com/Nucs/ogedit/commit/99b33af35aaaf1b506d01dafafa8202523d11e88) · 2 files changed, 83 insertions(+)

Documents shortcut names, keys, modifiers and customization examples; no product-code change.

Changed paths: `CLAUDE.md` (M), `README.md` (M).

### 30. 1d85463 — Suppress dead_code warning for safe_start field

2025-11-29T18:49:50+02:00 · [commit](https://github.com/Nucs/ogedit/commit/1d854632e39c2f8a69c1de1acfd1f0ab6ab98325) · 1 file changed, 1 insertion(+)

Allows the now-unused EditInfo.safe_start field after the wrapped-edit fix; lint-only change.

Changed paths: `src/buffer/mod.rs` (M).

### 31. 3c28123 — Fix clippy warnings

2025-11-29T18:54:40+02:00 · [commit](https://github.com/Nucs/ogedit/commit/3c2812355c7e46283c4c71f4dc0f1d0e5220db23) · 3 files changed, 18 insertions(+), 20 deletions(-)

Lint-only parser/reload/watcher simplification (strip_prefix, direct returns, iteration).

Changed paths: `src/bin/edit/config.rs` (M), `src/bin/edit/documents.rs` (M), `src/bin/edit/watch.rs` (M).

### 32. 1b220e4 — Fix clippy warnings

2025-11-29T19:01:50+02:00 · [commit](https://github.com/Nucs/ogedit/commit/1b220e4109d9631a082a5352a4465d008334a243) · 1 file changed, 3 insertions(+), 3 deletions(-)

Completes watcher iteration lint fixes; no new feature.

Changed paths: `src/bin/edit/watch.rs` (M).

### 33. 7ee6888 — Actively check disk on F5 reload instead of relying only on watcher

2025-11-29T19:23:18+02:00 · [commit](https://github.com/Nucs/ogedit/commit/7ee6888e4850a2b76d1f0e117eb66e305a22b208) · 1 file changed, 2 insertions(+), 1 deletion(-)

F5 checks current disk metadata as well as the watcher cache before deciding which confirmation to show.

Changed paths: `src/bin/edit/draw_editor.rs` (M).

### 34. 3f95547 — Add documentation and comprehensive unit tests for new features

2025-11-29T20:02:06+02:00 · [commit](https://github.com/Nucs/ogedit/commit/3f95547dee31b1c8f870612127a8c493559a4945) · 8 files changed, 621 insertions(+), 18 deletions(-)

Adds settings/hotkey/history and buffer highlighting/line-helper tests, serial watcher CI step and docs. Also expands menu/dialog/clipboard logging.

Changed paths: `.github/workflows/ci.yml` (M), `CLAUDE.md` (M), `README.md` (M), `src/bin/edit/config.rs` (M), `src/bin/edit/draw_menubar.rs` (M), `src/bin/edit/draw_statusbar.rs` (M), `src/bin/edit/main.rs` (M), `src/buffer/mod.rs` (M).

### 35. 396c111 — Add line number click selection feature

2025-11-29T21:14:20+02:00 · [commit](https://github.com/Nucs/ogedit/commit/396c1114911d81edbb865e9f2038e16363cbd8b8) · 8 files changed, 71 insertions(+), 1 deletion(-)

Adds line-number click, Shift+click and drag selection in the TUI, explicitly credited to Microsoft Edit PR #622 by jenia90; expands dialog logging.

Changed paths: `README.md` (M), `src/bin/edit/draw_editor.rs` (M), `src/bin/edit/draw_filepicker.rs` (M), `src/bin/edit/draw_menubar.rs` (M), `src/bin/edit/draw_statusbar.rs` (M), `src/bin/edit/main.rs` (M), `src/bin/edit/state.rs` (M), `src/tui.rs` (M).

### 36. e0371c6 — Add cross-platform installers and native archive formats

2025-11-29T21:48:39+02:00 · [commit](https://github.com/Nucs/ogedit/commit/e0371c6508aa22f949af3c5aa7c0b6aefa089af7) · 2 files changed, 426 insertions(+), 37 deletions(-)

Adds macOS PKG/TAR.GZ and Linux DEB/RPM/AppImage/TAR archives plus checksums and release-note instructions.

Changed paths: `.github/workflows/release.yml` (M), `CLAUDE.md` (M).

### 37. 9c35dab — Fix YAML syntax error in CI workflow

2025-11-29T21:51:51+02:00 · [commit](https://github.com/Nucs/ogedit/commit/9c35dab1a7cc7d1177c82369d7971c1c0b6b365f) · 1 file changed, 1 insertion(+), 1 deletion(-)

Quotes the CI command containing watch:: to fix YAML parsing.

Changed paths: `.github/workflows/ci.yml` (M).

### 38. de573b8 — Run flaky file watcher tests only on Linux

2025-11-29T21:53:44+02:00 · [commit](https://github.com/Nucs/ogedit/commit/de573b883986689f3374ee1bbc16d5fdc24a392f) · 1 file changed, 2 insertions(+), 1 deletion(-)

Temporarily limits native watcher integration tests to Linux because of Windows/macOS timing failures; superseded by 067ffe7.

Changed paths: `.github/workflows/ci.yml` (M).

### 39. 067ffe7 — Fix macOS file watcher test failures

2025-11-29T21:57:59+02:00 · [commit](https://github.com/Nucs/ogedit/commit/067ffe7b4098c1b484254944af712bcf1c8b06fb) · 2 files changed, 13 insertions(+), 5 deletions(-)

Deletes the kqueue filter before closing an unwatched descriptor, increases command-settling wait, and re-enables watcher tests on all platforms.

Changed paths: `.github/workflows/ci.yml` (M), `src/bin/edit/watch.rs` (M).

### 40. 7fc67cf — Improve file watcher test reliability on Windows

2025-11-29T22:02:20+02:00 · [commit](https://github.com/Nucs/ogedit/commit/7fc67cfa72f894905cec654f3ea9f058d90fb948) · 6 files changed, 1208 insertions(+), 12 deletions(-)

Increases Windows watcher waits/retries and diagnostics, enables full backtraces; accidentally includes design notes, a shortcut and scratch test files.

Changed paths: `.github/workflows/ci.yml` (M), `docs/design-mark-highlights.md` (A), `ogedit.exe.lnk` (A), `src/bin/edit/watch.rs` (M), `test_reload.txt` (A), `test_reload_feature.sh` (A).

### 41. dee789e — Remove accidentally committed test files

2025-11-29T22:02:52+02:00 · [commit](https://github.com/Nucs/ogedit/commit/dee789efdbb9732d8f8c26d2a013b1f834dc6d40) · 4 files changed, 1183 deletions(-)

Removes the four accidental scratch files from 7fc67cf. They exist in history but not in the final fork delta.

Changed paths: `docs/design-mark-highlights.md` (D), `ogedit.exe.lnk` (D), `test_reload.txt` (D), `test_reload_feature.sh` (D).

### 42. 8e578c4 — Release v1.2.3

2025-11-30T08:42:27+02:00 · [commit](https://github.com/Nucs/ogedit/commit/8e578c417fcf08004b96b9516718e8038f2f0afe) · 1 file changed, 1 insertion(+), 1 deletion(-)

Bumps Cargo.toml version to 1.2.3; no feature code changes.

Changed paths: `Cargo.toml` (M).

### 43. 9ae091e — Release v1.2.4

2025-11-30T11:31:17+02:00 · [commit](https://github.com/Nucs/ogedit/commit/9ae091ec07bde356f8dfc4199d499fe4266f5692) · 3 files changed, 7 insertions(+), 5 deletions(-)

Bumps Cargo.toml/lockfile to 1.2.4 and extracts appimagetool before building to avoid the CI FUSE requirement.

Changed paths: `.github/workflows/release.yml` (M), `Cargo.lock` (M), `Cargo.toml` (M).

### 44. 4a7beb0 — Add auto-tagging workflow for releases

2025-12-03T08:03:52+02:00 · [commit](https://github.com/Nucs/ogedit/commit/4a7beb08ba702e53e635ce599927279991bfa113) · 2 files changed, 82 insertions(+), 10 deletions(-)

Adds Cargo-version auto-tag workflow and changes release version extraction to Cargo.toml. The historical GITHUB_TOKEN tag push does not trigger the release workflow.

Changed paths: `.github/workflows/release.yml` (M), `.github/workflows/version-tag.yml` (A).

### 45. e69f3a2 — Handle race condition in auto-tag workflow

2025-12-03T08:12:22+02:00 · [commit](https://github.com/Nucs/ogedit/commit/e69f3a2c603620baea002aef4a8d3442d9af4bf9) · 1 file changed, 12 insertions(+), 2 deletions(-)

Adds auto-tag concurrency and treats all failed pushes as tag races, hiding real push failures; repaired during the legacy audit.

Changed paths: `.github/workflows/version-tag.yml` (M).

### 46. 01cf53f — Add optional EDITOR env var setting to installers

2025-12-03T19:40:40+02:00 · [commit](https://github.com/Nucs/ogedit/commit/01cf53f500831993d59d677fef92bc224b2e34bd) · 14 files changed, 466 insertions(+), 24 deletions(-)

Adds opt-in EDITOR setup in MSI/PKG/DEB and the RPM helper, templates/scripts and release integration. Default remains off; audit found lifecycle/privilege issues in scripts.

Changed paths: `.github/workflows/release.yml` (M), `CLAUDE.md` (M), `deb/config` (A), `deb/postinst` (A), `deb/postrm` (A), `deb/templates` (A), `pkg/Distribution.xml.template` (A), `pkg/conclusion.html` (A), `pkg/license.html` (A), `pkg/postinstall-editor-env` (A), `pkg/welcome.html` (A), `rpm/ogedit-editor.sh.sample` (A), `rpm/ogedit-set-editor` (A), `wix/main.wxs` (M).

## Later Microsoft history, separate from fork features

This complete upstream range is included to make the second comparison auditable. It is a change inventory, not a claim that these commits have been merged into OGEdit. The legacy-code review checks overlapping risk areas separately. In particular, later upstream undo/redo, single-line paste, zero-width replacement, terminal sanitization, large-file and Unix I/O fixes deserve comparison before any future rebase.

| Commit | Date | Microsoft change |
| --- | --- | --- |
| [`5f284a1`](https://github.com/microsoft/edit/commit/5f284a1df9266ab07bb068e5eff6d7844b651a97) | 2025-12-01 | Move arena & helpers into their own crate (#694) |
| [`1601c3f`](https://github.com/microsoft/edit/commit/1601c3f592a042e8c331fe6184361fe0d627db8a) | 2025-12-02 | i18n: Add Arabic translations (#634) |
| [`ca661a2`](https://github.com/microsoft/edit/commit/ca661a2901f0b92fd51a1772f801af445d98fa74) | 2025-12-01 | i18n: Add Estonian translations (#693) |
| [`d71e94b`](https://github.com/microsoft/edit/commit/d71e94b3035d2ac27c3d5f2d3d5e02fd444048a5) | 2025-12-02 | Improve Korean translations (#663) |
| [`3df9e7c`](https://github.com/microsoft/edit/commit/3df9e7cb6cc2b454af03651241541f0604b84419) | 2025-12-01 | Fix multiple issues found under Linux (#706) |
| [`364579a`](https://github.com/microsoft/edit/commit/364579a0456348c6f0e5f84c83d86d48751790e7) | 2025-12-02 | Add Indonesian translation (#629) |
| [`fa2b1b8`](https://github.com/microsoft/edit/commit/fa2b1b88cc6fe4b28362311af0d43bfa540ab30e) | 2026-01-05 | Update man page for edit command to version 1.2.1 (#727) |
| [`dbd7465`](https://github.com/microsoft/edit/commit/dbd74656a9cae4f097a1d8d7d4e3b42e3df5e615) | 2026-01-05 | Fix colors with Terminal.app's Clear Dark theme (#728) |
| [`48c2ab2`](https://github.com/microsoft/edit/commit/48c2ab29498699feb65d2f3c9cbf80e084d64bc7) | 2026-01-13 | Use platform line ending when opening single line files (#739) |
| [`d0db071`](https://github.com/microsoft/edit/commit/d0db071474c193785aacafe426498bee43f6b2ae) | 2026-01-20 | Create parent directories when saving to a non-existent path (#738) |
| [`8d9b1ed`](https://github.com/microsoft/edit/commit/8d9b1ede11eecc98d0df6ecae9af06a00f6a6aae) | 2026-01-20 | Add proper multithreading support to Arena (#741) |
| [`6b6a4b2`](https://github.com/microsoft/edit/commit/6b6a4b2d9fb83a7c2513506325e80b8a17200f0a) | 2026-01-21 | Add a JSON parser (#742) |
| [`9bc86c5`](https://github.com/microsoft/edit/commit/9bc86c5acdc8e9ffe86dabee68626e87724457af) | 2026-01-22 | Add a glob matcher (#743) |
| [`ad8415f`](https://github.com/microsoft/edit/commit/ad8415fd44e1630716e902505869a8ef33305758) | 2026-01-23 | Clean up error handling (#745) |
| [`e5a92eb`](https://github.com/microsoft/edit/commit/e5a92eb7d672b82387b4bb6e00f8d24f591a1cb9) | 2026-01-26 | Fix the latest clippy warning (#751) |
| [`524b22c`](https://github.com/microsoft/edit/commit/524b22c3c5eb9a81a527e3bb40f8a97518d8371f) | 2026-01-28 | Fix warnings when building on macOS (#754) |
| [`14c1def`](https://github.com/microsoft/edit/commit/14c1defa25b5e8ce857b81a5dabce7807e24775a) | 2026-02-12 | Remove the majority of nightly features (#761) |
| [`719a478`](https://github.com/microsoft/edit/commit/719a478cf017d70221a6a8388c37e1bef672e0af) | 2026-02-12 | Change copyright in draw_menubar.rs (#760) |
| [`6c79a1d`](https://github.com/microsoft/edit/commit/6c79a1d7e392a45a6e4d95a8ac81db0186fcc157) | 2026-02-12 | Remove allocator_api (#762) |
| [`ff45b14`](https://github.com/microsoft/edit/commit/ff45b14d5e7200556e020db5a8b9bdb2434852a2) | 2026-02-14 | BVec::push_encode_utf16, Some micro optimizations (#764) |
| [`017c0ba`](https://github.com/microsoft/edit/commit/017c0ba30de458f9707f65265be1f99a4508fc4c) | 2026-03-19 | Fix an incorrect debug assert (#774) |
| [`baa3cb0`](https://github.com/microsoft/edit/commit/baa3cb05b4738c0fe2275d11b2eb2be3cfe7fc40) | 2026-03-19 | Use natural sort for filenames (#763) |
| [`8ad89c8`](https://github.com/microsoft/edit/commit/8ad89c849dd93940c2784b7a3cee67734c7d667a) | 2026-03-20 | Introduce the LSH compiler (#753) |
| [`0bfb7a3`](https://github.com/microsoft/edit/commit/0bfb7a321008cba1c47c149c63a248f4d667349d) | 2026-03-23 | Add the LSH runtime (#776) |
| [`fa709ba`](https://github.com/microsoft/edit/commit/fa709bade0c51430db208998a770b8b8c55d9007) | 2026-03-25 | Integrate LSH into edit (#777) |
| [`d1908b0`](https://github.com/microsoft/edit/commit/d1908b0b5317e43f747b5ecc86ef67a7035e30fb) | 2026-03-25 | Add rudimentary settings.json support (#779) |
| [`05452ab`](https://github.com/microsoft/edit/commit/05452ab90ce7fa7b6161ca003307828bf168bef2) | 2026-03-27 | Complete the initial LSH implementation (#624) |
| [`4cef4ea`](https://github.com/microsoft/edit/commit/4cef4eacdaf80e7c2218d2c4366754d026bfb4d5) | 2026-03-27 | lsh: fix up PowerShell a bit (#784) |
| [`8b24907`](https://github.com/microsoft/edit/commit/8b24907aabb5b36edac064025d404df2b692ac6f) | 2026-03-27 | Add a small fuzzing harness for LSH (#782) |
| [`5820364`](https://github.com/microsoft/edit/commit/5820364ce484ba58ce0de98abbb62cd9f02655b4) | 2026-03-30 | Prepare for v2.0.0 (#785) |
| [`c92c166`](https://github.com/microsoft/edit/commit/c92c166ef6a3a5c4ea31ba13418c5cce229bcec2) | 2026-03-30 | Fix the Azure pipeline (#787) |
| [`35b863f`](https://github.com/microsoft/edit/commit/35b863f826ec7a7a55f0833ddc57f2e4c8038b6d) | 2026-03-31 | lsh: fix strings in PowerShell (#790) |
| [`6468920`](https://github.com/microsoft/edit/commit/6468920b16c85ed0ee6e57ae74a002b364863d7f) | 2026-04-07 | Add Shell, Python and JavaScript highlighting (#795) |
| [`7c0b3d1`](https://github.com/microsoft/edit/commit/7c0b3d17c1d566aac4198117798b7c120eaea633) | 2026-04-16 | clippy: fix ptr_as_ptr lint (#730) |
| [`c157ff3`](https://github.com/microsoft/edit/commit/c157ff328086a2e3a5afbec0956c0666b88d10f0) | 2026-04-17 | feat(tools): add one-line Linux installer (curl&#124;bash) with ICU symlink shim + docs (#628) |
| [`d887414`](https://github.com/microsoft/edit/commit/d887414dabb764b555eea34085e6469176f606f6) | 2026-04-18 | Handle horizontal scrolling on macOS touchpads (#794) |
| [`d4d3f3b`](https://github.com/microsoft/edit/commit/d4d3f3b6dca2c969b425da3005fb65d602ef990f) | 2026-04-21 | Fix Azure pipeline (again) (#807) |
| [`330d345`](https://github.com/microsoft/edit/commit/330d345a5da6ae485a3503d2ad518381d7ab20df) | 2026-04-21 | Align LSH language IDs with VS Code (#806) |
| [`23db2ba`](https://github.com/microsoft/edit/commit/23db2ba93e22131bb096566c311648f7deb14f62) | 2026-04-21 | Improve LSH documentation (#805) |
| [`9e7b938`](https://github.com/microsoft/edit/commit/9e7b938e9e771a51bbdaf9bbe0b4e8213ff2460f) | 2026-04-23 | Fix reading redirected input from stdin (#810) |
| [`c8a9787`](https://github.com/microsoft/edit/commit/c8a9787607b354ea1a73ac73e918b6618c496389) | 2026-04-23 | Fix scrolling in modals (#812) |
| [`8ed8551`](https://github.com/microsoft/edit/commit/8ed85518f0fdec1958a681573089711e5b706884) | 2026-04-27 | Further improve LSH docs (#813) |
| [`2800cbb`](https://github.com/microsoft/edit/commit/2800cbb2ef57d3e5330fcfb04204a8c246dc6315) | 2026-04-27 | lsh: Fix PowerShell keyword lists (#820) |
| [`24e2d79`](https://github.com/microsoft/edit/commit/24e2d79b879cc6f015f25aa363f7da03115133f9) | 2026-04-27 | Fix multi-line pastes into single-line fields (#818) |
| [`de3db9b`](https://github.com/microsoft/edit/commit/de3db9b10ae65db01d884c68b1cb850e2b3193ad) | 2026-04-27 | Further improve xterm mouse decoding (#819) |
| [`e5f0c45`](https://github.com/microsoft/edit/commit/e5f0c4574871f5a351e3bac44ddfddf07e3ec479) | 2026-04-27 | Stop moving focus into the search box (#821) |
| [`2bb7c10`](https://github.com/microsoft/edit/commit/2bb7c1090ac12fa3c039ce5a3884a67aff148c8e) | 2026-04-27 | Fix stability of compare_strings_ascii (#823) |
| [`d14e24b`](https://github.com/microsoft/edit/commit/d14e24b349b36d5a143c0c76d9dffd2c2b968028) | 2026-04-28 | Fix zero-width regex replace (#815) |
| [`d3f8697`](https://github.com/microsoft/edit/commit/d3f86975dc3c1298bf7300dbdf409a1df8d8b2b7) | 2026-04-28 | Fix text search on *nix (#824) |
| [`c481b71`](https://github.com/microsoft/edit/commit/c481b718da336ed43adbbfdf543b0368a24fe740) | 2026-04-29 | Update Turkish translations (#833) |
| [`58408a7`](https://github.com/microsoft/edit/commit/58408a72d2dcd8a47e6fb91e709ca9216e7b8f26) | 2026-04-30 | update and correct zh-hant translation (#830) |
| [`f7de351`](https://github.com/microsoft/edit/commit/f7de3513efb267a5ca60046fd8385673e74f2848) | 2026-04-29 | Fix localization selection (#835) |
| [`3afb4f7`](https://github.com/microsoft/edit/commit/3afb4f7fb2576b054d271938760243ba62a12a12) | 2026-04-29 | Support both Serbian languages (Cyrillic & Latin) (#749) |
| [`9642f7f`](https://github.com/microsoft/edit/commit/9642f7ffca7026c3c4fe8ef54061201b7344a3b0) | 2026-05-04 | Update Polish translations (#838) |
| [`cc5f7ef`](https://github.com/microsoft/edit/commit/cc5f7ef8b8f22e96e7255b37051dca35301ff51c) | 2026-05-04 | Add macOS Homebrew install instructions (#839) |
| [`737450c`](https://github.com/microsoft/edit/commit/737450cef9ee40030221d1ddc4e91a5e67173306) | 2026-05-05 | Fix stdarch_loongarch support in stdext (#837) |
| [`428ef55`](https://github.com/microsoft/edit/commit/428ef55715ff747782759e309a37e5ce8e49010a) | 2026-06-09 | Fix zh-Hant locale fallback (#846) |
| [`10cbfcc`](https://github.com/microsoft/edit/commit/10cbfcc7330c894f2173611029df44ca5cb6fd77) | 2026-06-30 | Load settings before parsing args (#875) |
| [`c5d5455`](https://github.com/microsoft/edit/commit/c5d54558e09b44934b5dbf7c7df640818fef4b31) | 2026-07-22 | LoongArch SIMD: use immediate compare and simplify accumulator setup (#858) |
| [`e04a670`](https://github.com/microsoft/edit/commit/e04a6708fe24744d119944cc90839c7c45659c12) | 2026-07-22 | Fix release pipeline (#897) |
| [`ed52eff`](https://github.com/microsoft/edit/commit/ed52eff8ac2a0f7cdca74c05517f338d4e675a17) | 2026-07-22 | Use copied() instead of cloned() for u16 iterator (#884) |
| [`e16e2a4`](https://github.com/microsoft/edit/commit/e16e2a40c1e231bcf9f086b361a68cccf2d2b31c) | 2026-07-22 | Add TOML highlighting (#866) |
| [`885ef2e`](https://github.com/microsoft/edit/commit/885ef2e8f8f01afe643ef4eb939c03619fd06848) | 2026-07-22 | Add Java highlighting (#898) |
| [`b5303d7`](https://github.com/microsoft/edit/commit/b5303d7d9639f236b997f57b024b009384012034) | 2026-07-27 | input: Translate \0 as Ctrl+Space (#899) |
| [`843e48f`](https://github.com/microsoft/edit/commit/843e48f54427c8f1f1166d800a5e08e043bd695a) | 2026-07-27 | Require --goto for file positions (#900) |
| [`b05f727`](https://github.com/microsoft/edit/commit/b05f727e52cf1583a3104673783143af7bc5aa42) | 2026-07-28 | Fix two LSH issues (#901) |
| [`ebc3f7a`](https://github.com/microsoft/edit/commit/ebc3f7a735ae5d83ce34e5356e7e360f649e3f41) | 2026-07-28 | Fix large_const_arrays warning (#902) |
| [`9c9fdcc`](https://github.com/microsoft/edit/commit/9c9fdcc135cbf5c2f850f57988e5b6490c903e73) | 2026-07-28 | Add Rust highlighting (#809) |
| [`774a02f`](https://github.com/microsoft/edit/commit/774a02f705b80140e62da2f112bd1b865ff91e0c) | 2026-07-28 | Add Lua highlighting (#800) |
| [`68d54e4`](https://github.com/microsoft/edit/commit/68d54e435fb75a188c80257c2f73079e1a29749f) | 2026-07-29 | Add C# highlighting (#802) |
| [`6704781`](https://github.com/microsoft/edit/commit/67047813fc6e600830aba00e0bd2c4afafd120a7) | 2026-07-30 | Add F# highlighting (#857) |
| [`60a47dc`](https://github.com/microsoft/edit/commit/60a47dccf28a6fb740278da19b9fdf8841ccbc43) | 2026-07-30 | Add Ruby highlighting (#849) |
| [`b2fac44`](https://github.com/microsoft/edit/commit/b2fac443bb967c42d7b6a4aa37a04fc3b843007e) | 2026-07-30 | Fix backtracking in alternations (#903) |
| [`5700117`](https://github.com/microsoft/edit/commit/570011725492fc6196d4a314bf928e0ddd076eaa) | 2026-07-30 | Add C/C++ highlighting (#796) |
| [`33ae9b0`](https://github.com/microsoft/edit/commit/33ae9b0632f6e26f6a7981ec88f537d66701e7cd) | 2026-07-30 | Add assert_unchecked for memchr2's return value (#836) |
| [`5366d29`](https://github.com/microsoft/edit/commit/5366d29363de17cf9976f4bbfc0f00a9be2bc926) | 2026-07-30 | Fix case-insensitive matching with uppercase literals (#850) |
| [`874ef3e`](https://github.com/microsoft/edit/commit/874ef3e437baebbc9a897d287423b34a819121ce) | 2026-07-31 | Add Go highlighting (#859) |
| [`a6b0f42`](https://github.com/microsoft/edit/commit/a6b0f4239c3f127a0cec55d717390e7d8e4ebe98) | 2026-07-31 | Use Cargo v3 resolver (#905) |
| [`6301ef3`](https://github.com/microsoft/edit/commit/6301ef3268baf4bda96dda682c58f3d018bf874b) | 2026-07-31 | Check clippy on workspace packages (#906) |
| [`729d6ae`](https://github.com/microsoft/edit/commit/729d6aed08d7e2c15fa407da5e2051126a96c499) | 2026-07-31 | Add Odin highlighting (#863) |
| [`3e20615`](https://github.com/microsoft/edit/commit/3e2061572a30c6d039dbd8bf1e56d82eea63d8df) | 2026-08-01 | Support negative line numbers for goto (#841) |
| [`0a6f8a3`](https://github.com/microsoft/edit/commit/0a6f8a33bb2d7110a36434956aef023338cde326) | 2026-08-03 | Select lines when clicking/dragging on line numbers (#622) |
| [`11f0848`](https://github.com/microsoft/edit/commit/11f0848e02bec715122adb8c295f1e1a87edec3c) | 2026-08-03 | Correctly parse colours on big endian systems (#721) |
| [`a6eeb20`](https://github.com/microsoft/edit/commit/a6eeb205c68906cf858cefa2467c55f64f887493) | 2026-08-03 | Scroll sideways while holding the shift key and scroll fast while holding the alt key (#766) |
| [`721d848`](https://github.com/microsoft/edit/commit/721d84815f485ba9cb2a2cd6b1de038d36469caa) | 2026-08-04 | Don't auto-scroll horizontal drag-selection in the top/bottom rows (#895) |
| [`93dfdf9`](https://github.com/microsoft/edit/commit/93dfdf99296a58708fea3166f693074d683e1852) | 2026-08-07 | Use compatible checkbox marker glyphs (#840) |
| [`9b34f18`](https://github.com/microsoft/edit/commit/9b34f1876966d834453cba39968f97137d0471b4) | 2026-08-07 | Toggle menus on click (#793) |
| [`d6ba99b`](https://github.com/microsoft/edit/commit/d6ba99bf3f961972d04ac55aae0f5e27a5db0c2f) | 2026-08-10 | Update Chinese translations (#910) |
| [`adf09b8`](https://github.com/microsoft/edit/commit/adf09b8b60013148cea34064836905816be6cf42) | 2026-08-11 | Sanitize all text before printing (#913) |
| [`01b3bc6`](https://github.com/microsoft/edit/commit/01b3bc64118c19482ae7f4d947ae149b19baf15b) | 2026-08-11 | Add Norwegian translations (#914) |
| [`7557c78`](https://github.com/microsoft/edit/commit/7557c7800ed856eaad51384d27b401f16f0c905e) | 2026-08-11 | Improve Ruby highlighting (#911) |
| [`22689f4`](https://github.com/microsoft/edit/commit/22689f47b82b8c99537a13939e4b24810e5519c7) | 2026-08-19 | Pin GitHub Actions to full-length commit SHAs (#920) |
| [`861419d`](https://github.com/microsoft/edit/commit/861419d10e69485c6479e772db4f9cef2822b8d4) | 2026-08-26 | Allow memset fallback to non SIMD implementation on x86 (#940) |
| [`4bfb9d9`](https://github.com/microsoft/edit/commit/4bfb9d956c5519338ae95e57bace6bcd692c7508) | 2026-08-27 | json: Double perf, Thirdle memory use (#941) |
| [`310d6e6`](https://github.com/microsoft/edit/commit/310d6e66c87657e9ce206c6d4bee226fb9cd7bc2) | 2026-08-27 | Fix a "chunk size must be non-zero" panic (#919) |
| [`826b4c0`](https://github.com/microsoft/edit/commit/826b4c097b6f14ba0a846dc56f2f0223a3aaf73a) | 2026-08-27 | Add an Inno Setup Installer (#915) |
| [`adc60a8`](https://github.com/microsoft/edit/commit/adc60a88d966d76944458294a550b3cd36e8b43e) | 2026-09-17 | Don't indent a line the selection stops at column 0 of (#918) |
| [`650edc1`](https://github.com/microsoft/edit/commit/650edc1c7467932aebeb4d1fc8de607bc40e1c6f) | 2026-09-18 | Unindent leading whitespace on backspace (#792) |
| [`8c97fa2`](https://github.com/microsoft/edit/commit/8c97fa2d95ac03403b738c77d6e10cfdac53218f) | 2026-09-18 | Theoretical fix for crashing when tabbing (#955) |
| [`b767559`](https://github.com/microsoft/edit/commit/b767559c323bebf95f5987a7fa66e79832291a5e) | 2026-09-18 | Configure CodeQL (#956) |
| [`f6ae615`](https://github.com/microsoft/edit/commit/f6ae6154576316a38854800f25e2d3c6a737dce6) | 2026-09-21 | Fix reading overly large files (#954) |
| [`2c4177f`](https://github.com/microsoft/edit/commit/2c4177f6ffb0752a677ea9bf8296beb9b79574e7) | 2026-09-21 | Turn lsh into a standalone crate (#958) |
| [`3cff89e`](https://github.com/microsoft/edit/commit/3cff89ee82ee1a7fd1a520449259f4bb8f364b8f) | 2026-09-21 | Enable CodeQL for actions (#959) |
| [`c470ca5`](https://github.com/microsoft/edit/commit/c470ca59af44c176ea39c672d09b32061c274896) | 2026-09-21 | Reintegrate stdext into edit (#960) |
| [`469e0e2`](https://github.com/microsoft/edit/commit/469e0e2648a1cb066488b07bdfae60c79898aae8) | 2026-10-02 | Add various file extensions (#945) |
| [`91244f5`](https://github.com/microsoft/edit/commit/91244f5f405a9bac289b98dabfcf55ae5866000b) | 2026-10-02 | Add Zig highlighting (#925) |
| [`8bb2038`](https://github.com/microsoft/edit/commit/8bb20388aca01287c404a1de64815606a88a5290) | 2026-10-02 | Add SQL syntax highlighting (#943) |
| [`731fc9a`](https://github.com/microsoft/edit/commit/731fc9a3b7e6421211c99e919114aa1206774864) | 2026-10-01 | Introduce utf8_decode_last (#969) |
| [`78cf564`](https://github.com/microsoft/edit/commit/78cf564a57ec9f957ff1c29b00a77c26f4ad7918) | 2026-10-01 | Improve YAML highlighting (#967) |
| [`c04475c`](https://github.com/microsoft/edit/commit/c04475cf38af0cb1e263bd93fb26601a8ade1639) | 2026-10-01 | Don't require grapheme boundaries (#970) |
| [`25fcee5`](https://github.com/microsoft/edit/commit/25fcee57c8e0422fa1f75390b1e45f6c0109aa09) | 2026-10-02 | Fix undo grouping in TextBuffer benchmarks (#971) |
| [`a359626`](https://github.com/microsoft/edit/commit/a359626098076da4a1a01dd93446d2b63b17a271) | 2026-10-05 | Improve Arena allocator perf (#974) |
| [`be94ecc`](https://github.com/microsoft/edit/commit/be94eccd7f27091783f56e3a96f7ac012a2ef36a) | 2026-10-06 | Make newline normalization undoable (#978) |
| [`6118429`](https://github.com/microsoft/edit/commit/61184294aa3e5058fd9724f6ba27f78e16ff8678) | 2026-10-06 | Fix several major undo/redo issues (#979) |
| [`cc1b35e`](https://github.com/microsoft/edit/commit/cc1b35e58e29c32bb138825e7bc71e63b099bd23) | 2026-10-06 | Introduce copious amounts of TextBuffer tests (#980) |
| [`e3e3b4c`](https://github.com/microsoft/edit/commit/e3e3b4c4d56a9dbd5e1586d3cf2e4f4848d72f26) | 2026-10-06 | Fix multiline strings for .js/.py/.sh (#973) |
| [`fdc3d14`](https://github.com/microsoft/edit/commit/fdc3d14ef87b4de8f7c9c6a6add6ba79cc486d63) | 2026-10-08 | unix: Fix EINTR handling in write_stdout (#982) |
| [`01df252`](https://github.com/microsoft/edit/commit/01df2525411e01fce951c229018cee2cf9aa6b36) | 2026-10-08 | unix: Avoid open() in file_id() (#983) |
| [`a820b05`](https://github.com/microsoft/edit/commit/a820b05544bd751a0f138e7b25ebcdddea29de1f) | 2026-10-08 | unix: Fix lang selection with an empty LANGUAGE env var (#984) |
| [`67543cb`](https://github.com/microsoft/edit/commit/67543cbfcdf24be10bfc1e36296f7a6a5843cd08) | 2026-10-08 | unix: Fix EINTR handling around termios (#985) |
| [`1be69c8`](https://github.com/microsoft/edit/commit/1be69c85a63bcd475a0b397196d0be3dbed60023) | 2026-10-09 | Fix lsh manifest (#988) |
| [`cb6de0a`](https://github.com/microsoft/edit/commit/cb6de0a8966aecd01d19a3541dab1829d0e679ca) | 2026-10-12 | python.lsh: Avoid capturing underscores as numeric literals (#981) |

## Verification scope

History inspection is read-only. Browser-driven TUI tests exercise the current integrated app against the surviving legacy behaviors; they do not establish that each historical commit was functional in isolation. Linux/macOS native watcher code replaced today is preserved in Git for reference, not silently claimed as tested on Windows. Native installer execution and release publication are not part of browser QA; the audit records local static/sandbox checks and remaining platform validation explicitly.
