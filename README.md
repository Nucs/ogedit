# ![Application Icon](./assets/edit.svg) OGEdit

A fast-paced fork terminal-based (TUI) text editor that extends [Microsoft Edit](https://github.com/microsoft/edit). 
OGEdit aims to deliver more features faster with less bureaucracy.


## Table of Contents

- [Key Features](#features)
  - [Selection Auto-Highlight](#selection-auto-highlight)
  - [Line Number Click Selection](#line-number-click-selection)
  - [Ctrl+D Duplicate Line](#ctrld-duplicate-line)
  - [File Watcher](#file-watcher)
  - [F5 Reload from Disk](#f5-reload-from-disk)
  - [Persistent Configuration](#persistent-configuration)
  - [Configurable Hotkeys](#configurable-hotkeys)
  - [Per-Project Folder Memory](#per-project-folder-memory)
  - [Recent Files](#recent-files)
  - [Debug Logging](#debug-logging)
  - [Data Directory](#data-directory)
- [Installation](#installation)
  - [From Source](#from-source)
  - [Build Requirements](#build-requirements)
- [Usage](#usage)
- [Binary Names](#binary-names)
- [Build Configuration](#build-configuration)
- [License](#license)
- [Acknowledgments](#acknowledgments)


![Screenshot of OGEdit](./assets/edit_hero_image.png)


## Features

### Selection Auto-Highlight

When you select text, all identical occurrences in the document are highlighted with a subtle yellow background. This makes it easy to see where a variable, function name, or text pattern appears throughout the file.

**Rules:**
- **2+ characters**: Always highlights all matches
- **1 character**: Only highlights if it's NOT a letter (symbols like `=`, numbers like `5` are highlighted; letters like `a` are not)
- **Whitespace-only**: No highlighting

**Performance:** Limited to 1000 matches to maintain responsiveness in large files.

### Line Number Click Selection

Click on line numbers in the left margin to quickly select entire lines:

- **Single click**: Selects the entire line
- **Shift+click**: Extends selection from current position to the clicked line
- **Click and drag**: Select multiple lines by dragging across line numbers

This feature only works when line numbers are visible (enabled by default, configurable via `line_numbers` setting).

*Based on [PR #622](https://github.com/microsoft/edit/pull/622) by [jenia90](https://github.com/jenia90).*

### Ctrl+D Duplicate Line

Duplicate the current line or selection with `Ctrl+D`. Also available via **Edit → Duplicate** menu (accelerator key: `D`).

- **No selection**: Duplicates entire current line below cursor
- **Full line selection**: Duplicates all selected lines below
- **Partial selection**: Duplicates selected text inline

### File Watcher

OGEdit monitors open files for external modifications using native OS APIs for instant detection:

| Platform | API |
|----------|-----|
| Windows | `ReadDirectoryChangesW` |
| Linux | `inotify` |
| macOS/BSD | `kqueue` |

When a file changes on disk:
- Status bar shows **[Modified]** indicator
- Press `F5` or use **File → Reload** to update
- Falls back to timestamp polling if native watching is unavailable

### F5 Reload from Disk

Press `F5` to reload the current file from disk. A confirmation dialog appears showing:

- **File changed externally**: Shows what will happen if you reload (lose local changes or just refresh)
- **Unsaved local changes**: Warns before discarding your edits
- **Cursor position**: Automatically restored after reload using smart line matching

The reload check also verifies disk state directly, catching changes that the file watcher might miss.

**Cursor Restore Algorithm:**
1. Saves the content of the current line before reload
2. Searches for that exact line in the reloaded content
3. If found uniquely, moves cursor to that line
4. If multiple matches exist, picks the one closest to the original line number
5. Falls back to clamping cursor position to valid bounds if line not found

### Persistent Configuration

Settings are saved to `~/.ogedit/state.json` and restored on startup:

| Setting | Description | Default |
|---------|-------------|---------|
| `word_wrap` | Enable word wrap for long lines | `false` |
| `indent_with_tabs` | Use tabs instead of spaces for indentation | `false` |
| `tab_size` | Tab width in spaces (1-8) | `4` |
| `newline_crlf` | Use CRLF line endings | `true` on Windows, `false` elsewhere |
| `line_numbers` | Show line numbers in left margin | `true` |
| `line_highlight` | Highlight the current line | `true` |
| `insert_final_newline` | Add newline at end of file when saving | `true` on Unix, `false` on Windows |
| `ruler_column` | Vertical ruler position (0-255, 0=disabled) | `0` |
| `hotkeys` | Keyboard shortcuts (see [Configurable Hotkeys](#configurable-hotkeys)) | (defaults below) |
| `project_folders` | Per-project last-used save folders (auto-managed) | `{}` |
| `recent_files` | Recently opened files with timestamps (auto-managed, max 100) | `[]` |

Changes via status bar or View menu are saved automatically. The config file supports `//` comments.

### Configurable Hotkeys

All keyboard shortcuts are customizable in `state.json` under the `hotkeys` object:

```json
"hotkeys": {
  "file_new": "Ctrl+N",
  "file_save": "Ctrl+S",
  "edit_duplicate_line": "Ctrl+D"
}
```

**Format:** `"action_name": "Modifier+Key"`

**Available modifiers:** `Ctrl`, `Alt`, `Shift` (combine with `+`)

**Available keys:** `A-Z`, `0-9`, `F1-F24`, `Space`, `Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`

| Action | Description | Default |
|--------|-------------|---------|
| `file_new` | Create new file | `Ctrl+N` |
| `file_open` | Open file | `Ctrl+O` |
| `file_save` | Save file | `Ctrl+S` |
| `file_save_as` | Save As | `Ctrl+Shift+S` |
| `file_reload` | Reload from disk | `F5` |
| `file_close` | Close document | `Ctrl+W` |
| `file_exit` | Exit application | `Ctrl+Q` |
| `edit_undo` | Undo | `Ctrl+Z` |
| `edit_redo` | Redo | `Ctrl+Y` |
| `edit_cut` | Cut | `Ctrl+X` |
| `edit_copy` | Copy | `Ctrl+C` |
| `edit_paste` | Paste | `Ctrl+V` |
| `edit_duplicate_line` | Duplicate line | `Ctrl+D` |
| `edit_find` | Find | `Ctrl+F` |
| `edit_replace` | Replace | `Ctrl+R` |
| `edit_find_next` | Find next | `F3` |
| `edit_select_all` | Select all | `Ctrl+A` |
| `view_go_to_file` | Go to file / Recent files | `Ctrl+P` |
| `view_go_to_line` | Go to line | `Ctrl+G` |
| `view_word_wrap` | Toggle word wrap | `Alt+Z` |

Invalid or missing hotkeys fall back to defaults.

### Per-Project Folder Memory

OGEdit remembers the last folder you saved to, per project:

- **Project**: The working directory where the editor was launched
- **Behavior**: When you save a file via Save As, the folder is remembered
- **Persistence**: Next time you open OGEdit in the same project, the Save As dialog defaults to the remembered folder
- **Fallback**: If the saved folder no longer exists, falls back to current working directory

This is useful when working on projects where you frequently save files to a specific subfolder.

### Recent Files

OGEdit tracks recently opened files (up to 100) for quick access:

- **Access:** Press `Ctrl+P` (Go to File) or use **View → Go to File**
- **Display:** Shows currently open documents, then a separator, then recent files
- **Filtering:** Only shows files that exist on disk and are not currently open
- **Opening:** Click or press Enter on a recent file to open it immediately
- **Persistence:** Recent files are stored in `state.json` with timestamps

This makes it easy to quickly reopen files you were recently working on.

### Debug Logging

Logs user interactions to `~/.ogedit/logs/` for debugging:

**File naming:** `{sanitized_cwd}_{YYYYMMDD}_{pid}.log`
- `{sanitized_cwd}`: Working directory with path separators replaced by `--`
- `{YYYYMMDD}`: Date in compact format
- `{pid}`: Process ID for uniqueness across instances

**Tracked events:**
- Text input, paste, cut, copy, delete operations
- Keyboard shortcuts and menu interactions
- File operations (open, save, new, close)
- Search and replace operations
- Cursor movements and selections (with byte offsets)
- Mouse clicks with target area
- Settings changes
- Panics and errors

**Log format:** `[HH:MM:SS.mmm] EVENT_TYPE: details`

```
[12:34:56.789] CURSOR_MOVE: Ln 5, Col 10 (offset 142) -> Ln 8, Col 15 (offset 247) [navigation]
[12:34:56.789] PASTE: "Hello, World!\nSecond line"
[12:34:56.789] MOUSE_CLICK: (15,8) [left] -> editor
```

### Data Directory

```
~/.ogedit/
├── state.json           # Configuration file (JSON with // comments)
├── state.json.backup    # Backup of corrupted config (if recovery triggered)
└── logs/                # Debug logs per session
    └── myproject_20251128_12345.log
```

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/Nucs/ogedit.git
cd ogedit

# Install Rust nightly
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

# Build release (~246 KB binary)
cargo build --config .cargo/release-nightly.toml --release

# Binary location: target/release/ogedit.exe (Windows) or target/release/ogedit (Unix)
```

### Build Requirements

- **Rust nightly** toolchain with `rust-src` component
- **Windows**: Visual Studio Build Tools (MSVC toolchain)
- **Linux/macOS**: GCC or Clang

## Usage

```bash
ogedit                    # Open editor
ogedit file1.txt file2.txt  # Open files
ogedit --version          # Show version
```

## Binary Names

- **Primary:** `ogedit` (or `ogedit.exe` on Windows)
- **Alternative:** `ogmsedit` (to avoid naming conflicts with other `edit` commands)

## Build Configuration

| Environment Variable | Description |
|---------------------|-------------|
| `EDIT_CFG_LANGUAGES` | Comma-separated list of languages to include (see `i18n/edit.toml`) |
| `EDIT_CFG_ICUUC_SONAME` | Custom ICU library name (e.g., `libicuuc.so.76`) |
| `EDIT_CFG_ICU_RENAMING_VERSION` | ICU version suffix for symbols |

## License

MIT License - See [LICENSE](./LICENSE)

## Acknowledgments

OGEdit is maintained by [EliBelash](https://github.com/Nucs).
