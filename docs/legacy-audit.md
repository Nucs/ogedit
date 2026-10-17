# OGEdit legacy audit

The original fork contains **46 commits changing 40 final paths**. This audit maps every commit, reviews the surviving feature code and delivery scripts, reproduces representative defects in the old executable, and fixes them in the current integrated app. Browser QA runs the real Windows terminal editor through ConPTY; it is not a web reimplementation. The historical executable used unchanged 01cf53f source; its stale lockfile package version alone was reconciled from 1.2.3 to 1.2.4 to build offline. Final validation and platform limits are recorded explicitly below. “Failproof” is an engineering aim, not a claim that every environment or failure sequence has been proven impossible.

## Authority and scope

This is the authoritative summary of the **legacy fork audit**. The [complete commit map](legacy-commit-map.md) and [machine-readable history](legacy-commits.json) are authoritative for commit provenance. Focused [editing](legacy-editing-review.md), [configuration](legacy-config-review.md), [persistence](legacy-persistence-review.md), and [delivery](legacy-delivery-review.md) reviews provide implementation evidence. Where a focused review says a finding was reported for integration, the integration status in this document supersedes that interim note. [Workbench documentation](workbench.md) covers the separately implemented modernization; [earlier validation](validation.md) records those earlier runs. `CLAUDE.md` is historical guidance, not a current statement of version, binary size, test totals or watcher implementation.

| Boundary | Exact reference | Meaning |
| --- | --- | --- |
| Microsoft fork ancestor | [`82cc84a610ded54d57f956285a8c4307c74bf364`](https://github.com/microsoft/edit/commit/82cc84a610ded54d57f956285a8c4307c74bf364) | Common ancestor, 2025-10-31. |
| OGEdit before today's work | [`01cf53f500831993d59d677fef92bc224b2e34bd`](https://github.com/Nucs/ogedit/commit/01cf53f500831993d59d677fef92bc224b2e34bd) | Committed v1.2.4 snapshot, 2025-12-03. Today's work was uncommitted at audit start and is excluded by SHA. |
| Microsoft before today's cutoff | [`cb6de0a8966aecd01d19a3541dab1829d0e679ca`](https://github.com/microsoft/edit/commit/cb6de0a8966aecd01d19a3541dab1829d0e679ca) | Latest upstream commit before 2026-10-17 00:00 Asia/Jerusalem, equivalent to 2026-10-16 21:00 UTC. |
| Actual OGEdit contribution | `82cc84a..01cf53f` | 46 commits; 40 final paths; 7,887 inserted and 281 deleted lines. |
| Later divergent upstream comparison | `01cf53f` → `cb6de0a` | 198 paths; 20,658 insertions and 12,208 deletions in that direction. Microsoft has 122 commits after the ancestor. This is not the fork contribution or a claim that all later Microsoft changes were merged/reviewed as OGEdit features. |

The [fork patch](legacy-audit/fork-changes.patch), [Microsoft-at-cutoff → old-OGEdit snapshot patch](legacy-audit/upstream-at-cutoff-to-old-ogedit.patch), and [40-path inventory](legacy-audit/changed-files.tsv) make the comparison reproducible. Files added and then removed within the history are still listed per commit, although they do not appear in the final 40-path delta. Commit titles were checked against diffs: `940f30b` introduced duplication as well as About changes; `1af1a9b` added only Ctrl+D dispatch despite its test claims; `39d4f92` added panic/startup/resize logging as well as documentation.

## Stable area and capability map

IDs below are stable references for follow-up work. **Operational** means a representative primary path is verified; **partial** means implementation exists but identified validation remains. Each status is bounded by the manual and automated evidence tables, not a universal quality score.

| Area | Purpose and boundary | Capabilities | Current status / remaining measurable outcome |
| --- | --- | --- | --- |
| A1 — Identity and provenance | Fork identity, attribution, historical evidence and documentation authority. | CAP-ID-01 fork name/About/license; CAP-ID-02 complete commit/path provenance. | Operational: About attribution observed and all historical commits mapped. Keep upstream-origin features correctly credited. |
| A2 — Editing and selection | Byte-buffer changes, selection ranges, wrap geometry and undo. Does not own file persistence. | CAP-EDIT-01 exact duplication; CAP-EDIT-02 gutter line selection; CAP-EDIT-03 matching-selection highlights; CAP-EDIT-04 wrapped replacement. | Operational representative paths: baseline/fixed duplication and gutter comparison, Undo and occurrence highlights observed. Wrapped Unicode replacement/duplication/Undo also passed; retain layout/byte regression tests. |
| A3 — Settings and command routing | Persistent defaults, configured commands, focus precedence and logical macro capture. | CAP-CFG-01 validated durable settings; CAP-CFG-02 all 20 legacy commands; CAP-CFG-03 wrap/ruler/default lifecycle. | Operational representative paths: remapped commands, wrap persistence and displayed defaults observed. Ensure host-reserved keys and modal focus stay explicitly bounded. |
| A4 — File navigation | Recent-file presentation and last Save As folder per launch directory. | CAP-NAV-01 bounded MRU; CAP-NAV-02 per-project folder memory. | Operational: Save As folder survives restart with a file argument; canonical recent/workspace aliases show one result and reopen correctly. Parser and history invariants have regression coverage. |
| A5 — Disk changes and reload | Watched paths, disk baselines, save/reload commit boundaries and cursor restoration. | CAP-DISK-01 watcher lifecycle; CAP-DISK-02 confirmed reload/cursor/encoding; CAP-DISK-03 non-destructive save/recovery integration. | Operational representative paths: reload/cancellation/missing-file/cursor/legacy-encoding checks passed. Preserve user edits until successful save/reload commits. |
| A6 — Diagnostics | Explicit debug logging, formatting and failure handling; never document recovery. | CAP-LOG-01 opt-in tracing; CAP-LOG-02 safe Unicode/binary/panic formatting. | Operational representative paths: Unicode paste/copy and opt-in traces passed. Logging retention and large snapshots remain documented costs. |
| A7 — Delivery | Version/tag provenance, build verification, packaging and optional EDITOR lifecycle. | CAP-REL-01 release identity/retries; CAP-REL-02 installer lifecycle; CAP-REL-03 reproducible tool inputs. | Partial: 18 fixture tests, actionlint and x64 MSI compilation pass. Native package lifecycle and hosted release workflow execution remain unverified. |

Implementation anchors: A1 [About/menu](../src/bin/edit/draw_menubar.rs), [license](../LICENSE), [history](legacy-commit-map.md); A2 [buffer](../src/buffer/mod.rs), [TUI](../src/tui.rs); A3 [config](../src/bin/edit/config.rs), [shortcut resolver](../src/bin/edit/shortcuts.rs), [composition/dispatch](../src/bin/edit/main.rs); A4 [file picker](../src/bin/edit/draw_filepicker.rs), [file palette](../src/bin/edit/workbench.rs); A5 [documents](../src/bin/edit/documents.rs), [watcher](../src/bin/edit/watch.rs), [safe save](../src/bin/edit/safe_save.rs), [recovery](../src/bin/edit/recovery.rs); A6 [logger](../src/bin/edit/logging.rs), [state helpers](../src/bin/edit/state.rs); A7 [delivery review](legacy-delivery-review.md), [release workflow](../.github/workflows/release.yml), [delivery tests](../tools/test_delivery.py).

The runtime flow is terminal input → focused TUI/application command resolution → buffer transaction → rendering. File operations cross a separate disk boundary through the document manager. Watch events and metadata polling mark disk versions for comparison; they do not silently replace unsaved text. Settings, recent paths, recovery and optional logs live under the user data directory (`OGEDIT_DATA_DIR` isolates QA). Release automation crosses a separate repository/package-manager boundary and was not executed against GitHub or the host's native installers merely to validate UI behavior.

## Historical work versus today's changes

| Capability | Before today | Earlier today, before this audit | New audit work |
| --- | --- | --- | --- |
| Core terminal editor | Microsoft supplied renderer, gap buffer, SIMD/Unicode, undo, search, file I/O, encoding, wrap and status bar. | Modernization retained these foundations. | Reviewed fork-facing integration; no claim of reimplementing Microsoft's editor. |
| Duplication, gutter and selection highlights | Fork introduced duplication/highlights; gutter selection was ported from Microsoft PR #622 by jenia90. | Shared edit transactions, multicursors and streaming gap-safe matching were already present. | Exact selection/blank-line/EOF/newline/Undo fixes; inclusive gutter ranges; Unicode highlight rules; wrapped layout repair. |
| Settings and shortcuts | Persistent defaults and 20 configurable fields existed, with incomplete dispatch and permissive parsing. | Atomic config write and isolated data directory already existed. | Strict parser/backup/MRU fixes; complete focus-aware dispatch; logical macro capture follows resolved commands; keyboard wrap persists; rulers survive named-file and Save As transitions. |
| Recent files / Save As folder | Legacy MRU and per-launch-directory folder memory. | Ctrl+P gained workspace search; file paths gained quoting/separator normalization, native Browse and live folder preview. | MRU ordering/cap/deduplication corrected; remembered project folder no longer loses priority merely because a file was opened on the command line. |
| Watch/reload/save | Custom native watcher, timestamp checks, F5 confirmations and line-based cursor restoration; direct writes/reload into live buffers. | Notify parent watches, metadata/content baselines, staged saves, temporary-buffer reload, crash recovery and comparisons. | Whole-line nearest cursor match, current-encoding F5 reload and safe rejection of untitled reload. |
| Logging | Always-on content logging and idle snapshots. | Logging became opt-in with state snapshot guards. | Disabled calls skip content formatting; UTF-8 truncation cannot split characters; binary escaping, reentrant panic logging and custom shortcut names repaired. |
| Delivery | Release/tag workflows and Windows/macOS/Linux packages, with optional EDITOR. | Toolchain/dependencies and ordinary CI checks had been modernized. | Tag identity/retry/permissions fixes; safer package scripts, quoting/checksums and pinned AppImage tooling; local fixture/validator/MSI checks. |
| Other new features | Not legacy fork features. | Editable workspace search; multicursor/macros; structured CSV/JSON/log views; LSH syntax/brackets; command palette/workspace fuzzy lookup. | Preserved while legacy fixes were integrated; LSH is later Microsoft work, not an original OGEdit invention. |

## Concrete repairs and evidence

| Capability | Defect and current correction | Evidence |
| --- | --- | --- |
| CAP-EDIT-01 | Column-zero partial multiline selection expanded to whole lines; blank lines could be lost; empty EOF duplication did nothing; newline normalization/Undo selection changed source meaning. Duplication now uses an exact checked byte transaction and restores the original selection. | [Editing review and regression names](legacy-editing-review.md#historical-defects-corrected-in-this-audit); baseline/fixed browser comparison below. |
| CAP-EDIT-02 | Upward/downward gutter drags selected inconsistent portions/terminators. Both directions now select inclusive logical lines, including wrapped continuations and an unterminated final line. | [TUI tests](../src/tui.rs); baseline/fixed browser comparison below. |
| CAP-EDIT-03 / CAP-EDIT-04 | Byte length misclassified single Unicode letters; wrapped multiline replacement undercounted visual rows. Grapheme eligibility and original edit-start layout accounting are corrected. | [Editing regression coverage](legacy-editing-review.md#regression-coverage). |
| CAP-CFG-01 | Raw-string JSON scanning accepted invalid tokens/nested setting confusion and failed Unicode/control escaping. Recovery could overwrite a prior backup. Full JSON parsing and exclusive synced backups now preserve corrupt bytes before replacement. | [Config review](legacy-config-review.md#findings-fixed-in-configrs); malformed/Unicode/BOM/repeated-backup tests. |
| CAP-CFG-02 | Several configured edit commands only changed labels; editor defaults consumed keys before app remaps. All 20 commands now resolve before default handling in the focused editor. Explicit remaps take priority over another command's default; modal input retains its own handling. Macro recording uses the resolved command, so a remapped Save cannot accidentally record Paste. | [Resolver/macro tests](../src/bin/edit/shortcuts.rs); [TUI focus test](../src/tui.rs); browser remap checks below. |
| CAP-CFG-03 | Keyboard Alt+Z could toggle wrap without persisting it; normal-file/Save As mode setup reset a configured ruler to zero. Both now preserve the intended settings lifecycle, with COMMIT_EDITMSG's special ruler retained. | [Dispatch](../src/bin/edit/main.rs), [document configured_ruler](../src/bin/edit/documents.rs); browser persistence/defaults evidence below. |
| CAP-NAV-01 | Same-second reopen order was wrong, history was unbounded, and raw/verbatim Windows aliases appeared twice. Updates move directly to front; existing paths share canonical identity before deduplication; loading keeps at most 100 entries. | [Config implementation/tests](../src/bin/edit/config.rs); final release browser MRU deduplication/reopen checks passed. |
| CAP-NAV-02 | Startup treated an opened file's parent as an explicitly requested directory, overriding the remembered project folder. The corrected order is explicit CLI directory → existing remembered folder → opened file parent → launch directory. | [Startup chooser and regression](../src/bin/edit/main.rs); final release saved-folder restart browser check passed. |
| CAP-DISK-02 | Reload matched substrings of longer lines and could forget a manually selected legacy encoding. F5 now retains encoding; nearest matching complete line wins with deterministic tie/clamp behavior. Untitled reload returns an error instead of unwrapping a missing path. | [Document reload and tests](../src/bin/edit/documents.rs), [nearest matching line](../src/buffer/mod.rs); reload/cancellation/missing-file/cursor/encoding browser checks passed. |
| CAP-LOG-02 | A long line with an emoji crossing byte 200 could crash even when no logger was enabled. Formatting now truncates at a UTF-8 boundary and disabled calls return immediately; binary escapes and panic-hook borrows are safe. | [Persistence review](legacy-persistence-review.md#confirmed-findings-repaired-in-loggingrs), [logging tests](../src/bin/edit/logging.rs). |
| CAP-REL-01 / CAP-REL-02 / CAP-REL-03 | Automatic tags did not trigger the intended release; push failures could be hidden as races; installer scripts could mishandle privileged user profiles or leave stale EDITOR settings. Workflows/scripts now validate identity, retry deliberately and preserve administrator-managed files. AppImage tools/runtime have checked pinned digests. | [Delivery findings and local evidence](legacy-delivery-review.md). |

## Coverage of all 40 final changed paths

This is a review-coverage ledger, not 40 claims of end-to-end runtime verification. “Inspected” denotes source/history review; executable evidence is named separately. Cross-area files are assigned a primary area for indexing. The [commit map](legacy-commit-map.md#every-fork-commit-oldest-first) records every introducing/modifying commit and transient file.

| Historical path | Area | Fork delta | Review/check coverage |
| --- | --- | --- | --- |
| [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) | A7 | +47/−4 | History and CI configuration review; centralized Rust checks recorded below. |
| [`.github/workflows/release.yml`](../.github/workflows/release.yml) | A7 | +727/−0 | Release identity, permissions, reusable invocation, package generation and integrity reviewed; actionlint and fixture tests. |
| [`.github/workflows/version-tag.yml`](../.github/workflows/version-tag.yml) | A7 | +72/−0 | Tag retry/race/error paths reviewed and exercised against disposable Git remotes. |
| `.github/workflows/winget.yml` (deleted) | A7 | +0/−35 | Historical deletion verified; no current WinGet publication capability claimed. |
| [`.gitignore`](../.gitignore) | A1 | +1/−0 | Fork rename/distribution exclusions inspected; no runtime behavior. |
| [`CLAUDE.md`](../CLAUDE.md) | A1 | +965/−0 | Historical claims compared with code; stale version, watcher and test figures are not current authority. |
| [`Cargo.lock`](../Cargo.lock) | A7 | +14/−14 | Historical dependency/version changes inventoried; current locked builds are separate validation. |
| [`Cargo.toml`](../Cargo.toml) | A7 | +6/−2 | Package identity, version, dependency and release-profile changes reviewed. |
| [`LICENSE`](../LICENSE) | A1 | +1/−0 | Microsoft license and added fork copyright preserved. |
| [`README.md`](../README.md) | A1 | +262/−69 | Fork feature claims checked against source and commit contents. |
| [`benches/lib.rs`](../benches/lib.rs) | A2 | +3/−3 | Crate-import rename inspected; no new benchmark result claimed. |
| [`deb/config`](../deb/config) | A7 | +8/−0 | Optional EDITOR/debconf flow reviewed; fixture tests. |
| [`deb/postinst`](../deb/postinst) | A7 | +37/−0 | Managed profile creation and administrator-file/symlink preservation tested in fixtures. |
| [`deb/postrm`](../deb/postrm) | A7 | +28/−0 | Remove/purge/upgrade cleanup behavior tested in fixtures. |
| [`deb/templates`](../deb/templates) | A7 | +9/−0 | Opt-in wording/default inspected. |
| [`i18n/edit.toml`](../i18n/edit.toml) | A1 | +107/−0 | Duplicate/reload strings and source wiring inspected; translations not independently linguistically certified. |
| [`pkg/Distribution.xml.template`](../pkg/Distribution.xml.template) | A7 | +57/−0 | Installer choices/platform/version flow and XML inspected; native installer not run. |
| [`pkg/conclusion.html`](../pkg/conclusion.html) | A7 | +18/−0 | Installation guidance and opt-in description inspected. |
| [`pkg/license.html`](../pkg/license.html) | A7 | +35/−0 | License presentation inspected. |
| [`pkg/postinstall-editor-env`](../pkg/postinstall-editor-env) | A7 | +38/−0 | Privilege boundary reviewed; unprivileged profile writer tested, native console-user discovery not executed. |
| [`pkg/welcome.html`](../pkg/welcome.html) | A7 | +17/−0 | Installer introduction and paths inspected. |
| [`rpm/ogedit-editor.sh.sample`](../rpm/ogedit-editor.sh.sample) | A7 | +4/−0 | Managed profile content inspected against helper behavior. |
| [`rpm/ogedit-set-editor`](../rpm/ogedit-set-editor) | A7 | +83/−0 | Enable/disable/status and administrator/symlink preservation tested in fixtures. |
| [`src/bin/edit/config.rs`](../src/bin/edit/config.rs) | A3 | +1445/−0 | Parser, corruption recovery, defaults, bindings and MRU invariants reviewed; regression tests. |
| [`src/bin/edit/documents.rs`](../src/bin/edit/documents.rs) | A5 | +124/−12 | Open/save/reload, disk baseline, encoding/cursor restoration and ruler lifecycle reviewed; regression tests. |
| [`src/bin/edit/draw_editor.rs`](../src/bin/edit/draw_editor.rs) | A2 | +199/−18 | Duplicate/search/reload/close dispatch and confirmation paths reviewed; browser and TUI tests. |
| [`src/bin/edit/draw_filepicker.rs`](../src/bin/edit/draw_filepicker.rs) | A4 | +80/−12 | Recent-file filters, Save As folder recording and file opening reviewed; browser checks and path tests. |
| [`src/bin/edit/draw_menubar.rs`](../src/bin/edit/draw_menubar.rs) | A3 | +140/−39 | Configured labels/actions, wrap persistence and About attribution checked; browser checks. |
| [`src/bin/edit/draw_statusbar.rs`](../src/bin/edit/draw_statusbar.rs) | A3 | +126/−19 | Indent/newline setting persistence and historical reload UI removal checked; browser checks. |
| [`src/bin/edit/localization.rs`](../src/bin/edit/localization.rs) | A1 | +3/−3 | Crate-rename diff inspected; inherited localization boundary retained. |
| [`src/bin/edit/logging.rs`](../src/bin/edit/logging.rs) | A6 | +730/−0 | Content formatting, disabled path, panic hook, timestamps and shortcut labels reviewed; regression tests. |
| [`src/bin/edit/main.rs`](../src/bin/edit/main.rs) | A3 | +180/−27 | Composition, input precedence, macro capture, file lifecycle and watcher dispatch reviewed; browser and unit tests. |
| [`src/bin/edit/state.rs`](../src/bin/edit/state.rs) | A5 | +237/−7 | Document/settings state, snapshot tracking, errors and reload integration reviewed. |
| [`src/bin/edit/watch.rs`](../src/bin/edit/watch.rs) | A5 | +1085/−0 | Historical native/fallback implementation compared with replacement; current lifecycle tests. |
| [`src/buffer/mod.rs`](../src/buffer/mod.rs) | A2 | +717/−5 | Duplicate, wrap, highlight and reload helpers reviewed; byte/layout/undo regression tests. |
| [`src/input.rs`](../src/input.rs) | A3 | +13/−7 | Historical shortcut display additions and current key representation inspected. |
| [`src/sys/unix.rs`](../src/sys/unix.rs) | A7 | +1/−1 | Historical function-pointer cast cleanup inspected; no Unix runtime claim from this Windows session. |
| [`src/tui.rs`](../src/tui.rs) | A2 | +54/−4 | Function-key menu labels, gutter selection and focus-aware shortcut hook reviewed; actual event-dispatch tests. |
| [`wix/License.rtf`](../wix/License.rtf) | A7 | +15/−0 | Installer license asset inspected and included in MSI build. |
| [`wix/main.wxs`](../wix/main.wxs) | A7 | +199/−0 | Features, PATH/EDITOR quoting and UI/package structure reviewed; x64 MSI compiles, not installed. |

## Browser-only manual QA

These observations were reported by the root audit from its real-terminal browser harness, using isolated fixtures/settings. The old `01cf53f` executable was used for the stated baseline comparisons. No native desktop automation was used. Only observed scenarios are marked verified; additional stress cases remain covered by the named regression tests.

| Scenario | Old baseline observation, where compared | Current observation | Status |
| --- | --- | --- | --- |
| Partial multiline Ctrl+D starting at column zero | Duplicated whole lines beyond the selected text. | Duplicates the exact selected text; Undo restores the original. | Verified |
| Upward gutter drag and replacement | Excluded part of the upper selected line and left unwanted text. | Inclusive complete-line selection; replacing it preserves the remaining last line; Undo restores. | Verified |
| Matching-selection highlights | Not separately compared in this run. | Selecting `alpha` highlights the other matching `alpha`. | Verified |
| About attribution | Legacy fork identity retained. | OGEdit and Microsoft fork attribution displayed. | Verified |
| Default Alt+Z persistence | Toggled view, but persisted `word_wrap` stayed false. | Persisted `word_wrap` becomes true. | Verified |
| Conflicting/default-overlapping shortcut remaps | Code review identified default textarea dispatch swallowing remaps. | Ctrl+S performs Undo; Ctrl+Z performs Save. | Verified |
| Remapped editing/view commands | Not separately compared. | F6 Copy, F7 Cut, F8 Paste, F9 Select All and F12 Wrap work. | Verified |
| Loaded settings and new-document defaults | Not separately compared. | Line numbers/current-line highlight disabled, tab width 2 and ruler 40 displayed as configured. | Verified |
| Save representation and folder recording | Not separately compared. | Saved bytes are exactly `abc\tx\r\nq\r\n`; project-folder JSON records the successful Save As directory. | Verified |
| Remembered folder after restarting in same launch directory | — | Final release restarted with a file argument and Save As displayed the remembered `legacy\save folder` directory. Explicit-directory precedence and missing-folder fallback also have a regression test. | Verified |
| Recent files reopen/order/filtering | — | Old raw/verbatim aliases produced two identical rows; final release migrated that history, showed one combined recent/workspace result, and reopened the intended file. MRU timing/order/cap have regression tests. | Verified |
| F5 external/local/both changes, cancellation, deletion and whole-line cursor restoration | — | External-only, local-only and combined warnings displayed; cancellation retained local edits; missing-file reload reported an error without losing edits; recreation succeeded; the cursor followed its unique line from row 2 to row 3. | Verified |
| BOM-less legacy encoding preserved by F5 | — | Reopened a BOM-less file as Windows-1252, changed its bytes externally, then F5 retained the encoding and displayed `café changed` correctly. | Verified |
| Long Unicode paste/copy, disabled logs and explicit trace output | — | Pasted and copied a long line with emoji crossing byte 200 with logging enabled; no crash; UTF-8 traces contain paste/copy/snapshot records. Default sessions created no log directory. | Verified |
| Wider wrap/paste/duplicate/Undo matrix | — | Pasted a long Unicode line across wrapped rows, replaced the selection with two lines, undid it, duplicated the full selection, and undid again; content and visual rows remained coherent. | Verified |

The actual saved sample above means bytes `61 62 63 09 78 0D 0A 71 0D 0A`, not literal backslash characters. It validates the tested tabs/CRLF/final-newline behavior, not every encoding or formatting combination.

## Automated and delivery validation

The final integrated suite passes **197 Rust tests** (92 library, 100 application, 2 explicitly ignored ICU, 3 documentation), plus **18 delivery regression tests**. The checks below ran after the relevant source changes; installer execution limits remain separate.

| Check | Result / evidence |
| --- | --- |
| Final Rust library/application suite | **92 library + 100 application tests pass**, `target/qa-legacy-tests.log`. |
| Explicitly ignored ICU tests | **2 pass** serially, `target/qa-legacy-ignored.log`. |
| Rust documentation tests | **3 pass**, `target/qa-legacy-doc.log`. |
| Clippy, all targets/features, warnings denied | **Pass**, `target/qa-legacy-clippy.log`. |
| Windows optimized release build | **Pass**, `target/qa-legacy-release.log`; 2,581,504-byte executable. |
| Linux x86_64 / macOS ARM64 compile checks | **Pass**, `target/qa-legacy-linux.log` and `target/qa-legacy-macos.log`; compilation, not native runtime QA. |
| Delivery regression fixtures | **18 pass**: real disposable Git operations, version/tag/race/retry validation, shell/profile lifecycle and XML checks. [Details](legacy-delivery-review.md#local-validation), `target/qa-legacy-delivery-tests.log`. |
| Workflow static validation | **Pass** with actionlint 1.7.12 plus Bash syntax checks. Optional external ShellCheck/Pyflakes integrations were not installed. |
| Windows x64 MSI compilation | **Pass** with WiX 5.0.2 and matching UI extension, using the existing release executable. `target/qa-legacy-msi-build.log`; this is package compilation, not installation. |
| Pinned AppImage downloads | Official assets matched both documented SHA256 digests. Linux executables were not run on this Windows host. |
| GitHub hosted release/tag workflow | **Not executed**; no remote release/tag was published for QA. |
| Native installers/package managers | **Not executed**: Windows install/upgrade/uninstall, ARM64 MSI, macOS PKG, Linux DEB/RPM/AppImage native lifecycle remain outside this run. |
| Final install/hash/launch command | **Updated and verified**: `%LOCALAPPDATA%\Programs\OGEdit\ogedit.exe`; SHA256 `F632F6FB834AADF28B3494538721565229E97BA53DB05AD32B739BDAA2B2A0C2`; installed `--version` returns 1.2.4. Previous executable retained; no user editor process terminated. |

## Invariants, boundaries and remaining work

- A2 owns byte/selection/Undo correctness; A3 must dispatch a resolved command once in the appropriate focus context. Macro capture must describe that same action, not the original physical key's default meaning.
- A5 must preserve the live buffer on failed reload and must not mark failed saves clean. Watch events are evidence to check a disk version, not permission to discard local edits. Save/reload content checks cannot provide a universal cross-process compare-and-swap against an uncooperative writer racing the final rename.
- A3 must preserve corrupt settings before replacement; invalid hotkeys fall back individually. Printable letters/digits/Space require Ctrl or Alt, and terminal/browser-reserved combinations may never reach the app. Settings remain last-writer-wins across simultaneous processes.
- A6 logging is explicitly opt-in and contains file paths/editor content when enabled. It has no retention limit; full idle snapshots have a cost on very large documents. Deep working-directory names can exceed log filename limits and disable file creation. Logging is not a substitute for recovery snapshots.
- A5 metadata-only polling cannot see same-length/same-mtime rewrites without a native event. The save baseline still hashes content to reject overwriting changed disk data.
- A7 fixture tests and Rust cross-compilation do not certify native package installation, system environment changes, distribution compatibility or GitHub runner behavior. The measurable next delivery gate is a real platform matrix covering install/upgrade/uninstall and a controlled hosted workflow run.

The native delivery gaps above remain explicit follow-up gates before a public multi-platform release. Source review, focused tests and representative browser comparisons establish the behaviors actually exercised; they do not justify an absolute “failure-proof” label.
