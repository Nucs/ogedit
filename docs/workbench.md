# OGEdit workbench

OGEdit keeps its terminal interface and byte gap buffer. Project tools edit the same
documents as the main editor, so edits remain unsaved until Save and participate in undo.

## Getting around

| Action | Shortcut |
| --- | --- |
| Commands | F2 or Ctrl+Shift+P |
| Files: open, recent, workspace | Ctrl+P |
| Workspace search | Ctrl+Shift+F, or Commands → Search workspace |
| Data view | Ctrl+Shift+L, or View → Data view |
| Record / stop macro | F4 or Ctrl+Shift+M |
| Replay last macro | Ctrl+Alt+M, or Commands → Replay last macro |
| Add next occurrence | Ctrl+Alt+D |
| Add cursor above / below | Ctrl+Alt+Up / Down |
| Rectangular selection | Alt+Shift+arrows or Alt+drag |
| Add a cursor with the mouse | Alt+click |
| Matching bracket | Ctrl+] |

Some terminal hosts cannot distinguish Ctrl+Shift+letter from Ctrl+letter. F2 and
the menus provide access to the same commands on those hosts. Modified keys are
decoded through the terminal's existing input protocol, including CSI-u and
modifyOtherKeys when the host supplies them.

On Windows, **File → Browse File** and **Browse** in the Open File dialog open
the native Windows file browser. It starts in the current file's folder or the
folder entered in Open File; cancelling keeps the current document and path.
Windows paths accept either slash style, including mixed separators and quoted
paths such as `"K:\projects\sample\README.md"`. Ctrl+P also accepts a pasted
path outside the workspace. A `:line[:column]` suffix can follow the path.

Entering an existing file path in **File name** immediately shows its containing
folder and highlights the file, scrolling it into view. The input keeps focus;
Enter opens the file. Relative paths stay anchored while the folder preview changes.

## Search and data

Workspace search respects ignore rules and searches open buffers, including unsaved
edits. Choose a result and press F2 or Enter to edit its source line. Apply with
Ctrl+Enter; Escape cancels. Source opens the original location. After applying an
edit, search refreshes against the updated buffers. Changing a source independently
invalidates captured results: refresh before editing again.

CSV and TSV views show decoded cells, including quoted delimiters and multiline
values. JSON/JSONC shows paths and values while preserving surrounding comments,
spacing, and number precision. Log views use named regex captures; Fields changes
the extraction pattern. Unmatched or absent fields remain read-only.

Use Up/Down to select a row, Left/Right to select a column, and F2 or Enter to edit.
The cell editor accepts multiline text. Applying an invalid JSON value or a stale
cell leaves the original untouched. Undo in the source editor reverses the complete
cell edit.

## Editing and macros

Multiple selections share one transaction for typing, deletion, paste, and undo.
Overlapping selections are normalized before editing. Column selection follows
display columns, accounting for tabs and wide Unicode characters.
Alt-clicks and zero-width column selections insert without replacing adjacent
characters. Extra insertion cursors are underlined; only actual selections have
a selected background.

Macros store logical editing commands and resolved paste text in `macros.json`.
They do not replay terminal escape sequences or file operations. Unsupported
actions stop recording before execution so an incomplete macro is not silently
saved. A replay is one undo step. If it exceeds buffer capacity, its edits roll back.
Saved macros are also available by name in Commands.

## Saves and recovery

Save writes a temporary sibling, syncs it, checks the disk version, and atomically
replaces the destination. Existing permissions and supported filesystem metadata
are preserved. Hard-linked destinations require Save As; symlinks resolve to their
target. Save As cannot overwrite another open document's buffer.

A changed disk file opens a comparison instead of silently overwriting it. Compare
with disk is also available in File and Commands. Reload asks before discarding
local edits; Save As keeps them in another file. Portable filesystems do not offer
a universal compare-and-swap rename, so an uncooperative writer can still race the
last check. Saves verify the committed content as well.

Unsaved buffers receive private, checksummed recovery snapshots about once a second.
Each running instance holds a session lock. A later instance offers abandoned
snapshots as unsaved copies, retaining encoding and newline mode without attaching
them to an original path. A crash can lose edits since the last completed snapshot.

Configuration, recovery, and macros live in `~/.ogedit`; `OGEDIT_DATA_DIR` overrides
that directory. Debug content logging is disabled unless `OGEDIT_LOG=1` is set.

## Resource bounds

- Workspace search: 8 MiB per file, 64 MiB of retained source snapshots, and 10,000
  result lines; the UI reports truncation or skipped files.
- Data views: 8 MiB of source, 20,000 rows, and 200,000 cells. Larger files remain
  editable in the normal source editor.
- File discovery: 200,000 candidates or a conservative 64 MiB storage budget.
  Discovery and workspace search share two worker permits. Cancellation releases
  captured state when the worker exits. Fuzzy queries are limited to 16 KiB and
  score matrices to 262,144 cells, with a bounded linear fallback.
- Syntax: upstream LSH definitions, incremental lexical checkpoints, and bounded
  work per frame. Lines over 1 MiB or excessive parser state fall back to plain
  rendering. Explicit bracket jumps search a bounded neighborhood of parsed text.
- Disk comparison: 32 MiB combined content and a bounded diff calculation.
- Macros: 10,000 commands and 1 MiB of captured text per recording.

## Building

The repository pins `nightly-2025-11-30` and the dependency lockfile. Run:

```sh
cargo test --locked --all-features --lib --bins
cargo test --locked --doc
cargo clippy --locked --all-features --all-targets -- --no-deps --deny warnings
cargo build --locked --config .cargo/release-nightly.toml --release
```

LSH is vendored from Microsoft Edit; its exact revision and small compatibility
patches are documented in `vendor/lsh/PROVENANCE.md`.
