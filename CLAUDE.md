# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

OGEdit is a terminal-based text editor that pays homage to MS-DOS Editor, built in Rust with a strong focus on small binary size and high performance. This is a fork/customization of Microsoft's Edit project. The editor can handle large files (1GB+) efficiently through SIMD-optimized operations and a unique architecture that avoids tracking line breaks in memory.

**Binary Names:**
- Primary: `ogedit` (or `ogedit.exe` on Windows)
- Alternative: `ogmsedit` (to avoid potential naming conflicts)

**Configuration:**
- Global settings are stored in `~/.ogedit/state.json`
- Configuration is automatically loaded on startup and saved when settings change
- Current configurable settings:
  - `word_wrap`: Whether Word Wrap is enabled by default (default: false)
  - `indent_with_tabs`: Use tabs (true) or spaces (false) for indentation (default: false)
  - `tab_size`: Tab width / spaces for indentation, 1-8 (default: 4)
  - `newline_crlf`: Use CRLF (true) or LF (false) for newlines (default: true on Windows)
  - `line_numbers`: Show line numbers in left margin (default: true)
  - `line_highlight`: Highlight the current line (default: true)
  - `insert_final_newline`: Add newline at end of file when saving (default: true on Unix)
  - `ruler_column`: Show vertical ruler at column, 0=disabled (default: 0)
  - `hotkeys`: Keyboard shortcuts (customizable, see below)
  - `project_folders`: Per-project last-used save folder mapping (auto-managed, do not edit manually)
  - `recent_files`: Recently opened files with timestamps (auto-managed, max 100 entries)
- Settings are automatically saved when changed via the status bar or View menu
- **Per-project folder memory:**
  - When you save a file using Save As, the editor remembers the folder you saved to
  - This is stored per-project, where "project" is the working directory where the editor was launched
  - Next time you open the editor in the same project, the Save As dialog will default to the last-used folder
  - This helps when working on projects where you frequently save files to a specific folder
- **Recent files:**
  - The editor tracks recently opened files (up to 100)
  - Access via **Ctrl+P** (Go to File) - shows open documents followed by recent files after a separator
  - Recent files are also shown in the **File > Open** dialog
  - Only files that exist and are not currently open are shown
  - Clicking a recent file opens it immediately
  - Files are sorted by most recently opened
- **Configurable Hotkeys:**
  - All keyboard shortcuts are customizable in `state.json` under the `hotkeys` object
  - Format: `"action_name": "Modifier+Key"` (e.g., `"file_save": "Ctrl+S"`)
  - Available modifiers: `Ctrl`, `Alt`, `Shift` (can combine multiple, e.g., `Ctrl+Alt+S`, `Ctrl+Shift+S`, `Ctrl+Alt+Shift+F1`)
  - Available keys: `A-Z`, `0-9`, `F1-F24`, `Space`, `Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`
  - **File operations:**
    - `file_new`: Create new file (default: `Ctrl+N`)
    - `file_open`: Open file (default: `Ctrl+O`)
    - `file_save`: Save file (default: `Ctrl+S`)
    - `file_save_as`: Save As (default: `Ctrl+Shift+S`)
    - `file_reload`: Reload from disk (default: `F5`)
    - `file_close`: Close document (default: `Ctrl+W`)
    - `file_exit`: Exit application (default: `Ctrl+Q`)
  - **Edit operations:**
    - `edit_undo`: Undo (default: `Ctrl+Z`)
    - `edit_redo`: Redo (default: `Ctrl+Y`)
    - `edit_cut`: Cut (default: `Ctrl+X`)
    - `edit_copy`: Copy (default: `Ctrl+C`)
    - `edit_paste`: Paste (default: `Ctrl+V`)
    - `edit_duplicate_line`: Duplicate line (default: `Ctrl+D`)
    - `edit_find`: Find (default: `Ctrl+F`)
    - `edit_replace`: Replace (default: `Ctrl+R`)
    - `edit_find_next`: Find next (default: `F3`)
    - `edit_select_all`: Select all (default: `Ctrl+A`)
  - **View operations:**
    - `view_go_to_file`: Go to file / Recent files (default: `Ctrl+P`)
    - `view_go_to_line`: Go to line (default: `Ctrl+G`)
    - `view_word_wrap`: Toggle word wrap (default: `Alt+Z`)
  - Invalid or missing hotkeys fall back to defaults
  - Example customization:
    ```json
    "hotkeys": {
      "file_save": "Ctrl+Shift+S",
      "file_save_as": "Ctrl+Alt+S",
      "edit_duplicate_line": "Ctrl+Shift+D"
    }
    ```
- **Corruption Handling:**
  - If `state.json` is corrupted or contains invalid JSON, it will be backed up to `state.json.backup`
  - A fresh configuration file with default values will be automatically created
  - Unknown fields in the JSON are ignored (forward compatibility)
  - Invalid values for known fields trigger corruption recovery

**Selection Auto-Highlight:**
- When text is selected, all identical occurrences are highlighted with a subtle yellow background
- **Highlighting rules:**
  - 2+ characters: always highlight
  - 1 character: only highlight if NOT a letter (symbols, numbers OK)
  - Whitespace-only: no highlight
- Performance capped at 1000 matches to maintain responsiveness
- Implementation: `find_all_matches()` in `src/buffer/mod.rs`

**File Watcher:**
- Cross-platform file watching using native OS APIs:
  - Windows: `ReadDirectoryChangesW`
  - Linux: `inotify`
  - macOS/BSD: `kqueue`
- Falls back to timestamp polling if native watching is unavailable
- Files are watched on open/save and unwatched on close
- Poll interval: ~1 second (60 frames)
- Implementation: `src/bin/edit/watch.rs`

**F5 Reload from Disk:**
- Press F5 (or use `file_reload` hotkey) to reload file from disk
- Shows confirmation dialog for:
  - External changes only: "File has been modified on disk"
  - Unsaved local changes: "You have unsaved changes"
  - Both: Combined warning with implications
- Cursor position is automatically restored after reload using smart line matching:
  1. Saves current line content before reload
  2. Searches for that line in the new content
  3. If unique match found, moves cursor there
  4. If multiple matches, picks closest to original line number
  5. Falls back to clamping if line not found
- Also performs direct disk check (catches changes watcher might miss)
- Implementation: `restore_cursor_after_reload()` in `src/buffer/mod.rs`

**Debug Logging:**
- Logs are written to `~/.ogedit/logs/` for debugging and tracing user interactions
- **Log file naming:** `{sanitized_cwd}_{YYYYMMDD}_{pid}.log`
  - `{sanitized_cwd}`: Working directory with path separators replaced by `--` (e.g., `c--users--john--project`)
  - `{YYYYMMDD}`: Date in compact format
  - `{pid}`: Process ID (ensures uniqueness when multiple instances run)
  - Example: `users--john--myproject_20251127_12345.log`
- **Logged events include:**
  - Application start/exit with session info (cwd, pid)
  - Startup sequence (terminal mode switch, files loaded from command line)
  - Terminal resize events
  - Text input and paste operations
  - Keyboard shortcuts (Ctrl+S, Ctrl+N, Ctrl+D, F5, etc.)
  - Menu clicks and checkbox toggles
  - File operations (new, open, save, close, reload)
  - File watcher events (external modifications detected)
  - Search/replace operations and option toggles
  - Settings changes (word wrap, encoding, newline type, indentation)
  - Dialog open/close with results
  - Document switching
  - Cursor movements (with line, column, and byte offset)
  - Text selections (with range, offsets, and selected content)
  - File picker interactions
  - Content snapshots (1 second after last change)
  - Panics/crashes (with location and message)
  - Error messages
- **Implementation:** `src/bin/edit/logging.rs`
- **Log format:** `[HH:MM:SS.mmm] EVENT_TYPE: details`
- **Cursor and selection logging:** Includes line, column, and byte offset information
  - **Mouse click (screen position + target area):**
    ```
    [12:34:56.789] MOUSE_CLICK: (15,8) [left] -> editor
    [12:34:56.789] MOUSE_CLICK: (5,0) [left] -> menubar
    [12:34:56.789] MOUSE_CLICK: (30,24) [left] -> statusbar
    ```
  - **Cursor movement from click (includes target):**
    ```
    [12:34:56.789] CURSOR_MOVE: Ln 5, Col 10 (offset 142) -> Ln 8, Col 15 (offset 247) [click:editor]
    ```
  - **Cursor movement from keyboard:**
    ```
    [12:34:56.789] CURSOR_MOVE: Ln 5, Col 10 (offset 142) -> Ln 5, Col 15 (offset 147) [navigation]
    ```
  - **Selection with content:**
    ```
    [12:34:56.789] SELECTION: Ln 1, Col 1 (offset 0) to Ln 3, Col 5 (offset 45) content="first line\nsecond line\nthird"
    ```
  - **Selection cleared:**
    ```
    [12:34:56.789] SELECTION_CLEAR
    ```
- **Content formatting:** Text content (paste, cut, copy, delete, text input, selections) uses adaptive formatting:
  - **< 256 bytes:** Escaped one-liner with special chars shown as `\n`, `\r`, `\t`, etc.
    ```
    [12:34:56.789] PASTE: "Hello, World!\nSecond line"
    ```
  - **>= 256 bytes:** Diff-style output with line numbers
    ```
    [12:34:56.789] PASTE: [CONTENT: 512 bytes, 15 lines]
      1| first line here
      2| second line
     ...
     15| last line
    ```
  - **Binary data (invalid UTF-8, >= 256 bytes):** Hex dump with offsets
    ```
    [12:34:56.789] PASTE: [BINARY: 512 bytes]
    00000000| 48 65 6c 6c 6f 20 57 6f  72 6c 64 21 0a 00 ff fe  |Hello World!....|
    ```
- **Content snapshots:** Full document content is logged 1 second after the last change (idle snapshot)
  ```
  [12:34:56.789] CONTENT_SNAPSHOT: doc="Untitled" "full document content here..."
  ```
  - Uses adaptive formatting (escaped one-liner for small content, diff-style for large content)
  - Only logged when document is idle (no changes in the last second)
  - Helps reconstruct document state timeline for debugging
- **Panic handling:** A panic hook is installed to capture crashes to the log
  - `logging::init()` is called FIRST in the startup sequence, before sys/arena/localization init
  - Logs panic message and source location before termination
  - Panic hook chain: logging hook → terminal cleanup hook (debug) → default hook
  - **Limitation:** In release builds with `panic = "abort"`, the hook may not run (immediate termination)

**Data Directory Structure:**
```
~/.ogedit/
├── state.json                                    # Configuration file
├── state.json.backup                             # Backup of corrupted config (if any)
└── logs/
    ├── users--john--project_20251127_12345.log   # Log per session
    └── ...
```

## Build and Test Commands

### Building

**Development builds:**
```bash
cargo build
```

**Release builds:**
- For Rust 1.90 or earlier:
  ```bash
  cargo build --config .cargo/release.toml --release
  ```
- For Rust nightly (1.91+):
  ```bash
  cargo build --config .cargo/release-nightly.toml --release
  ```

The release build configuration is highly optimized for binary size reduction using:
- `opt-level = "s"` (size optimization)
- LTO enabled
- `panic = "abort"` with `panic_immediate_abort`
- Symbol stripping
- Custom MSVC linker flags to avoid vcruntime140.dll dependency

### Testing

**Run all tests:**
```bash
cargo test
```

**Run ICU-related tests (requires proper ICU configuration):**
```bash
cargo test -- --ignored
```

### Running

```bash
cargo run -- [files...]
```

### Benchmarking

```bash
cargo bench
```

## Toolchain Setup and Prerequisites

### Initial Setup (Windows)

This project requires Rust nightly with specific components and Windows build tools. Here's the complete setup process:

#### 1. Install Rust

```bash
# Download and install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable

# Add cargo to PATH (or restart shell)
export PATH="$HOME/.cargo/bin:$PATH"
```

#### 2. Install Nightly Toolchain

The project uses `rust-toolchain.toml` to specify nightly, but you need to ensure it's installed:

```bash
# Install nightly toolchain
rustup toolchain install nightly

# Add rust-src component (required for release builds with build-std)
rustup component add rust-src --toolchain nightly-x86_64-pc-windows-msvc
```

#### 3. Install Visual Studio Build Tools (Windows MSVC target)

The project requires MSVC toolchain on Windows for linking and the winresource build dependency:

**Download and install:**
```bash
# Download VS Build Tools installer
curl -L "https://aka.ms/vs/17/release/vs_buildtools.exe" -o vs_buildtools.exe

# Install with C++ workload (requires admin privileges)
./vs_buildtools.exe --quiet --wait --norestart --nocache \
  --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended
```

This installs:
- MSVC compiler and linker (link.exe)
- Windows SDK
- C++ build tools

**Why it's needed:**
- The `winresource` build dependency (in `build/main.rs`) requires Windows resource compiler
- MSVC linker is needed for the MSVC target (default on Windows)
- Custom linker flags in `.cargo/release-nightly.toml` require MSVC toolchain

#### 4. Verify Installation

```bash
# Check Rust installation
rustc --version  # Should show nightly version
cargo --version

# Verify rust-src is installed
rustup component list --toolchain nightly | grep rust-src
```

### Common Issues and Solutions

#### Issue: "link.exe: command not found" or wrong link.exe

**Symptom:**
```
error: linking with `link.exe` failed: exit code: 1
note: link: extra operand '...'
```

**Cause:** Git's `/usr/bin/link.exe` is in PATH before MSVC's link.exe

**Solution:** Install Visual Studio Build Tools (see step 3 above). The Rust toolchain will automatically find the MSVC linker once installed.

#### Issue: "Missing manifest in toolchain 'nightly-x86_64-pc-windows-msvc'"

**Symptom:** Corrupted nightly installation

**Solution:**
```bash
# Remove corrupted toolchain
rm -rf "$HOME/.rustup/toolchains/nightly-x86_64-pc-windows-msvc"

# Reinstall
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly-x86_64-pc-windows-msvc
```

#### Issue: Release build fails with "unable to build with the standard library"

**Symptom:**
```
error: "...\\Cargo.lock" does not exist, unable to build with the standard library
try: rustup component add rust-src --toolchain nightly-x86_64-pc-windows-msvc
```

**Solution:** Install rust-src component (see step 2 above)

#### Issue: "Access is denied" when running executable

**Symptom:** Built executable cannot run from bash/PowerShell with exit code 5

**Solution:** This is a Windows security restriction. Options:
1. Run from cmd.exe or Windows Explorer directly
2. Configure Windows Defender to allow the executable
3. The tests via `cargo test` work fine and validate the build

### Alternative: GNU Toolchain (Advanced)

If you cannot install Visual Studio Build Tools, you can use the GNU toolchain:

```bash
# Install GNU nightly toolchain
rustup toolchain install nightly-x86_64-pc-windows-gnu

# Build with GNU toolchain
cargo +nightly-x86_64-pc-windows-gnu build
```

**Note:** This may require additional configuration and the `build/main.rs` script might need modification for the `winresource` dependency.

### Build Artifacts

After successful builds, executables are located at:
- Debug: `target/debug/ogedit.exe` (~1.1 MB with debug symbols)
- Release: `target/release/ogedit.exe` (~246 KB, highly optimized)

The release build is significantly smaller due to:
- `-Zbuild-std-features=panic_immediate_abort` (removes panic infrastructure)
- `opt-level = "s"` (optimize for size)
- LTO (link-time optimization)
- Symbol stripping

## Core Architecture

The codebase has a unique architecture centered around performance optimization:

### Text Buffer Design (src/buffer/)

**Critical design principle:** The text buffer does NOT track line breaks in memory. This permeates the entire codebase.

- Only the current cursor position is kept as state
- Navigation uses O(n) seeking through the document to count line breaks
- This enables exceptional performance through SIMD operations:
  - `src/simd/memchr2.rs`: Find next/previous line breaks at >100GB/s
  - `src/unicode/mod.rs`: UTF-8 iteration with U+FFFD replacement at ~4GB/s
  - `src/unicode/measurement.rs`: Grapheme cluster segmentation via `MeasurementConfig` at ~600MB/s
- Without word-wrap: memchr2 enables navigating 1GB files like 1MB files
- With word-wrap: still smooth thanks to optimized `MeasurementConfig`

### Key Modules

- **src/framebuffer.rs**: Implements a video game-style framebuffer
  - Draw UI to intermediate buffer, accumulating changes and handling color blending
  - Compare with previous frame and only send deltas to terminal

- **src/tui.rs**: Immediate mode UI implementation
  - Read the module documentation for architecture overview

- **src/vt.rs**: VT (Virtual Terminal) parser for terminal escape sequences

- **src/sys/**: Platform abstraction layer
  - `sys/windows.rs`: Windows-specific code
  - `sys/unix.rs`: Unix-specific code

- **src/bin/edit/**: Main application that ties everything together
  - ~90% UI code and business logic
  - `main.rs`: Entry point with `setup_terminal` function containing VT logic
  - `documents.rs`: Document management
  - `draw_*.rs`: Rendering modules for editor, filepicker, menubar, statusbar
  - `localization.rs`: i18n support
  - `state.rs`: Application state

### Performance-Critical Code Paths

When making changes, be aware these areas are heavily optimized:
- `src/simd/`: SIMD implementations for string operations
- `src/unicode/`: UTF-8 and grapheme cluster handling
- `src/buffer/`: Core text buffer operations

## Internationalization (i18n)

All translations are in `i18n/edit.toml`. At build time, `build/main.rs` processes this file and generates Rust code in `$OUT_DIR/i18n_edit.rs`.

**To limit languages in build:**
```bash
EDIT_CFG_LANGUAGES="en,de,fr" cargo build
```

Available languages are listed in `i18n/edit.toml` under `__default__`.

## ICU Configuration

The project optionally depends on ICU for Search and Replace functionality. Default SONAMEs:
- Windows: `icuuc.dll`, `icuin.dll`
- macOS: `libicucore.dylib` (both)
- Unix: `libicuuc.so`, `libicui18n.so`

**Configure custom ICU at build time:**
```bash
# Custom SONAME (e.g., versioned library)
EDIT_CFG_ICUUC_SONAME="libicuuc.so.76" cargo build

# Versioned exports
EDIT_CFG_ICU_RENAMING_VERSION="76" cargo build

# C++ prefixed exports (macOS default)
EDIT_CFG_ICU_CPP_EXPORTS="true" cargo build

# Auto-detect version (Unix default if no other options set)
EDIT_CFG_ICU_RENAMING_AUTO_DETECT="true" cargo build
```

## Development Guidelines

### Binary Size Priority

Keeping binary size small is a top priority. Generally:
- Do NOT add new dependencies without strong justification
- Use `#[cold]` and `#[inline]` attributes strategically
- Profile binary size impact of changes

### Nightly Features

This project requires Rust nightly and uses several unstable features:
- `allocator_api`
- `breakpoint`
- `cold_path`
- `linked_list_cursors`
- `maybe_uninit_*` features
- `stdarch_loongarch*` (for loongarch64 target)

See `src/lib.rs` for the complete list.

### Memory Management

The project uses a custom arena allocator (`src/arena/`):
- `scratch_arena()`: Temporary allocations
- `ArenaString`: Arena-allocated strings
- Scratch arena capacity: 128 MiB (32-bit), 512 MiB (64-bit)

### Terminal-Related Issues

If debugging terminal compatibility issues, check:
1. VT parser: `src/vt.rs`
2. Platform-specific code: `src/sys/windows.rs` or `src/sys/unix.rs`
3. Terminal setup: `setup_terminal()` in `src/bin/edit/main.rs`

## File Structure

```
src/
├── lib.rs                    # Library root with feature gates
├── bin/edit/                 # Main application binary
│   ├── main.rs              # Entry point and main loop
│   ├── state.rs             # Application state
│   ├── config.rs            # Global configuration (~/.ogedit/state.json)
│   ├── logging.rs           # Debug logging system (~/.ogedit/logs/)
│   ├── watch.rs             # Cross-platform file watcher
│   ├── documents.rs         # Document management
│   ├── draw_*.rs            # UI rendering modules
│   └── localization.rs      # i18n wrapper
├── arena/                    # Custom memory allocator
├── buffer/                   # Core text buffer (no line tracking!)
│   ├── gap_buffer.rs        # Gap buffer implementation
│   ├── line_cache.rs        # Line position caching
│   └── navigation.rs        # Cursor movement
├── simd/                     # SIMD-optimized operations
│   ├── memchr2.rs           # Fast line break search
│   ├── memset.rs            # Fast memory filling
│   └── lines_*.rs           # Line iteration
├── unicode/                  # UTF-8 and grapheme handling
│   ├── utf8.rs              # UTF-8 validation and iteration
│   ├── measurement.rs       # Grapheme segmentation
│   └── tables.rs            # Unicode property tables
├── sys/                      # Platform abstraction
│   ├── windows.rs           # Windows implementation
│   └── unix.rs              # Unix implementation
├── tui.rs                    # Immediate mode UI framework
├── vt.rs                     # VT escape sequence parser
├── framebuffer.rs           # Terminal rendering backend
├── document.rs              # Document model
├── input.rs                 # Input event handling
├── clipboard.rs             # Clipboard operations
└── [other utility modules]   # helpers, path, hash, fuzzy, etc.

build/
└── main.rs                   # Build script (i18n generation, ICU config)

i18n/
└── edit.toml                 # Translation strings
```

## Testing and Validation

### Test Suite

The project has comprehensive test coverage:

```bash
# Run all tests (unit + integration + doc tests)
cargo test

# Expected output:
# - ~36 unit tests (SIMD, Unicode, buffer operations)
# - ~1 integration test (document parsing)
# - ~3 doc tests (API examples)
# Total: ~40 tests, all should pass

# Run with verbose output
cargo test -- --nocapture

# Run specific test
cargo test test_name

# Run benchmarks
cargo bench
```

### ICU Tests (Optional)

Some tests are ignored by default because they require ICU library:

```bash
# Run ICU-related tests
cargo test -- --ignored

# These tests validate:
# - ICU library loading and symbol resolution
# - String comparison with locale support
```

### File Watcher Tests

The native file watcher tests are **ignored by default** because they can be flaky when run in parallel with other tests. This is due to Windows `ReadDirectoryChangesW` API timing issues - the OS doesn't guarantee immediate delivery of file change notifications.

```bash
# Run file watcher tests reliably (single-threaded)
cargo test watch:: -- --test-threads=1 --include-ignored

# Expected: 10 tests pass
# - 2 polling fallback tests (always reliable)
# - 8 native watcher tests (require single-threaded execution)
```

**Why flaky in parallel:**
- Race conditions with OS file notification APIs
- Event delivery timing is non-deterministic
- Multiple tests creating/modifying temp files can cause notification delays

**Polling fallback tests** (`test_polling_fallback_*`) always run and are reliable because they use timestamp-based detection.

### Continuous Integration Notes

For CI/CD pipelines on Windows:
1. Install Visual Studio Build Tools in CI environment
2. Ensure `rust-src` component is available
3. Run `cargo test` to validate builds
4. Release builds take ~2-3 minutes (rebuilding std library)

## Running the Application

### Command Line Usage

```bash
# Open editor without file
cargo run

# Open specific file(s)
cargo run -- file1.txt file2.txt

# Show help
cargo run -- --help

# Use release build for better performance
cargo run --release -- largefile.txt
```

### Executable Distribution

After building, the standalone executable can be distributed:

```bash
# Release build location
./target/release/ogedit.exe

# The executable is statically linked (on Windows with MSVC config):
# - No vcruntime140.dll dependency (statically linked)
# - Only depends on ucrtbase.dll (Universal CRT, part of Windows 10+)
# - Self-contained: no need to ship additional DLLs
```

**Distribution Note:** Include both binary names in portable packages:
- `ogedit.exe` - Primary binary name
- `ogmsedit.exe` - Copy of the same binary (alternative name to avoid conflicts)

### Distribution Checklist

When preparing binaries for release:

1. **Build with release config:**
   ```bash
   cargo build --config .cargo/release-nightly.toml --release
   ```

2. **Verify binary size:**
   - Should be ~246 KB (Windows x86_64 MSVC)
   - Significantly smaller than typical Rust binaries due to aggressive optimizations

3. **Test on clean system:**
   - Ensure no missing DLL dependencies
   - Verify terminal compatibility

4. **Package naming conventions:**
   - Primary binary: `ogedit` or `ogedit.exe`
   - Alternative name: `ogmsedit` or `ogmsedit.exe` (to avoid potential conflicts)
   - Distribution folder: `ogedit-v{VERSION}-{PLATFORM}-{ARCH}`
   - Example: `ogedit-v1.2.1-windows-x86_64.zip`

5. **Include assets (if distributing with installer):**
   - Icon: `assets/edit.ico`
   - Manifest: `src/bin/edit/edit.exe.manifest`
   - Both binaries: `ogedit.exe` and `ogmsedit.exe` (copy of same binary)

## Performance Profiling

For performance analysis:

```bash
# Build with profiling enabled
cargo build --release --config .cargo/release-nightly.toml

# Run benchmarks
cargo bench

# The benchmark suite tests:
# - SIMD operations (memchr, memset, line scanning)
# - Unicode operations (UTF-8 validation, grapheme segmentation)
# - Buffer operations (gap buffer, navigation)
```

## Environment Variables (Build-Time)

These affect how the binary is built:

```bash
# Limit included languages (reduces binary size)
EDIT_CFG_LANGUAGES="en,es,fr" cargo build --release

# ICU configuration (see ICU Configuration section)
EDIT_CFG_ICUUC_SONAME="..." cargo build
EDIT_CFG_ICU_RENAMING_VERSION="76" cargo build

# Use stable Rust features only (alternative to nightly)
RUSTC_BOOTSTRAP=1 cargo +stable build
# Note: Not officially supported, use at own risk
```

## Troubleshooting Checklist

If builds fail, verify:

1. ✅ Rust nightly installed: `rustup toolchain list | grep nightly`
2. ✅ rust-src component: `rustup component list --toolchain nightly | grep rust-src`
3. ✅ Visual Studio Build Tools (Windows): Check for MSVC in Program Files
4. ✅ Git link.exe not interfering: `where link.exe` should show MSVC path first
5. ✅ Clean build if switching configurations: `cargo clean`

If tests fail:
1. Check terminal compatibility (VT100/ANSI support)
2. Verify locale settings for Unicode tests
3. ICU tests are optional (ignored by default)

## Quick Start Summary

For first-time setup on Windows:

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. Install nightly and components
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

# 3. Install VS Build Tools (download installer first)
# See "Toolchain Setup and Prerequisites" section above

# 4. Build and test
cargo build                          # Debug build
cargo test                          # Run tests
cargo build --config .cargo/release-nightly.toml --release  # Release build

# 5. Run
cargo run -- myfile.txt
```

Expected build times:
- Debug build: ~20-30 seconds (first build), <1 second (incremental)
- Release build: ~2-3 minutes (first build, rebuilds std library)
- Tests: ~1 minute (includes downloading test dependencies)

## Version Management

### Bumping Version Numbers

When releasing a new version, update the version number in **ONE place**:

**File:** `Cargo.toml`
```toml
[package]
name = "ogedit"
version = "1.2.1"  # <-- UPDATE THIS
```

The version automatically propagates to:
- Binary output via `env!("CARGO_PKG_VERSION")` macro
- Version display: `ogedit --version` output
- About dialog in the UI
- No manual updates needed elsewhere in the code

### Release Checklist

When preparing a new release:

1. **Update version in Cargo.toml**
   ```bash
   # Edit Cargo.toml and change version = "X.Y.Z"
   ```

2. **Clean and rebuild**
   ```bash
   cargo clean
   cargo build --config .cargo/release-nightly.toml --release
   cargo test
   ```

3. **Create distribution package**
   ```bash
   mkdir -p ./dist/ogedit-vX.Y.Z-windows-x86_64
   cp ./target/release/ogedit.exe ./dist/ogedit-vX.Y.Z-windows-x86_64/
   cp ./target/release/ogedit.exe ./dist/ogedit-vX.Y.Z-windows-x86_64/ogmsedit.exe
   cp ./target/release/ogedit.pdb ./dist/ogedit-vX.Y.Z-windows-x86_64/
   cp ./README.md ./LICENSE ./dist/ogedit-vX.Y.Z-windows-x86_64/
   cp -r ./assets ./dist/ogedit-vX.Y.Z-windows-x86_64/

   # Create INSTALL.txt, SHA256SUMS.txt
   # Zip the distribution folder
   ```

4. **Test the distribution package**
   ```bash
   ./dist/ogedit-vX.Y.Z-windows-x86_64/ogedit.exe --version
   ./dist/ogedit-vX.Y.Z-windows-x86_64/ogmsedit.exe --version
   ```

5. **Generate checksums**
   ```bash
   cd ./dist/ogedit-vX.Y.Z-windows-x86_64
   sha256sum ogedit.exe ogmsedit.exe > SHA256SUMS.txt
   cd ..
   sha256sum ogedit-vX.Y.Z-windows-x86_64.zip > ogedit-vX.Y.Z-windows-x86_64.zip.sha256
   ```

### Version Numbering Scheme

OGEdit uses a **fork versioning** scheme based on Microsoft Edit's version:

```
{Edit Major}.{Edit Minor}.{OGEdit Release}
```

| Component | Description | Example |
|-----------|-------------|---------|
| Edit Major.Minor | Upstream Microsoft Edit version | `1.2` |
| OGEdit Release | OGEdit-specific release number | `.1`, `.2`, `.3` |

**Examples:**
- `1.2.1` = Based on Edit v1.2, first OGEdit release
- `1.2.2` = Based on Edit v1.2, second OGEdit release
- `1.3.1` = Based on Edit v1.3 (after upstream sync), first OGEdit release

When syncing with upstream Edit:
1. Merge/rebase upstream changes
2. Update version to `{new Edit version}.1`
3. Reset OGEdit release counter

Current version: **1.2.2** (based on Microsoft Edit 1.2)

## GitHub Actions CI/CD

The project uses GitHub Actions for continuous integration and releases.

### CI Workflow (`.github/workflows/ci.yml`)

Runs on every push/PR to `main`:

| Job | Platforms | Steps |
|-----|-----------|-------|
| `check` | Ubuntu, Windows, macOS | Tests, Clippy |
| `release-build` | Ubuntu, Windows, macOS | Verify release build works |

### Release Workflow (`.github/workflows/release.yml`)

Triggered by tags matching `release/v*` (e.g., `release/v1.2.1`).

**Build Targets:**

| Platform | Target | Portable | Installer (adds to PATH) |
|----------|--------|----------|--------------------------|
| Windows x64 | `x86_64-pc-windows-msvc` | `.zip` | `.msi` |
| Windows ARM64 | `aarch64-pc-windows-msvc` | `.zip` | `.msi` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.tar.xz`, `.tar.gz`, `.zip` | `.deb`, `.rpm`, `.AppImage` |
| macOS x64 | `x86_64-apple-darwin` | `.tar.gz`, `.zip` | `.pkg` |
| macOS ARM64 | `aarch64-apple-darwin` | `.tar.gz`, `.zip` | `.pkg` |

**Portable archives contain:**
- `ogedit` (or `ogedit.exe` on Windows)
- `ogmsedit` (alternative binary name)
- `README.md`
- `LICENSE`

**Windows MSI Installer:**
- Installs to `Program Files\OGEdit`
- Adds installation directory to system PATH
- Creates Start Menu shortcuts
- Optional: Desktop shortcut (enabled by default)
- Optional: Set EDITOR environment variable (disabled by default)
- Built using WiX Toolset v5
- Configuration: `wix/main.wxs`

**macOS PKG Installer:**
- Installs to `/usr/local/bin` (already on PATH)
- Includes both `ogedit` and `ogmsedit` binaries
- Optional: Set EDITOR environment variable (disabled by default, adds to ~/.zshrc or ~/.bash_profile)
- Built using native `productbuild` with Distribution.xml
- Configuration: `pkg/` directory

**Linux DEB Package (Debian/Ubuntu):**
- Installs to `/usr/bin`
- Install: `sudo dpkg -i ogedit-linux-x64.deb`
- Optional: Set EDITOR environment variable (prompts during installation via debconf, disabled by default)
- Configuration: `deb/` directory

**Linux RPM Package (Fedora/RHEL):**
- Installs to `/usr/bin`
- Install: `sudo rpm -i ogedit-linux-x64.rpm`
- Optional: Set EDITOR environment variable by running `sudo ogedit-set-editor --enable` after installation
- Configuration: `rpm/` directory

**Linux AppImage:**
- Universal portable format, runs on most Linux distributions
- No installation required: `chmod +x ogedit-linux-x64.AppImage && ./ogedit-linux-x64.AppImage`

**SHA256 checksums** are generated for each artifact (`.sha256` files).

**Prerelease detection:** Tags containing `-alpha`, `-beta`, or `-rc` are marked as prereleases.

### Creating a Release

When the user asks to "release" or "release vX.Y.Z", follow these steps:

**Step 1: Determine new version**
```bash
# Check current version
grep '^version' Cargo.toml
```

- If user specifies version: use that
- If user says "release": increment OGEdit release number (1.2.1 → 1.2.2)
- If user says "release beta": use next version with `-beta.1` suffix

**Step 2: Update Cargo.toml**
```toml
version = "1.2.2"  # New version
```

**Step 3: Commit and tag**
```bash
git add Cargo.toml
git commit -m "Release v1.2.2"
git tag release/v1.2.2
git push origin main
git push origin release/v1.2.2
```

**Step 4: Verify**
- GitHub Actions will build all 5 platforms
- Release appears at: `https://github.com/Nucs/ogedit/releases`

**Prerelease examples:**
```bash
# Beta release
git tag release/v1.2.2-beta.1

# Alpha release
git tag release/v1.2.2-alpha.1

# Release candidate
git tag release/v1.2.2-rc.1
```

Tags with `-alpha`, `-beta`, or `-rc` are automatically marked as prereleases.
