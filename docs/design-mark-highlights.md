# Design Document: Mark Highlights Feature

## Overview

Add two highlighting systems to OGEdit:

1. **Selection Auto-Highlight** - When text is selected, all identical occurrences are automatically highlighted. Deselecting clears them. (Like VS Code, Sublime Text)

2. **Persistent Marks** - Notepad++-style manual marks that survive navigation and editing, with multiple colors.

These are **separate systems** that coexist visually.

---

## Part 1: Selection Auto-Highlight

### User Experience

**Behavior:**
- Select any text → all identical occurrences in the document are highlighted
- Deselect (click elsewhere, move cursor) → highlights disappear
- No user action required beyond selecting text
- Works with any selection (word, partial word, multiple words, etc.)

**Visual:**
- Uses a distinct color from persistent marks (e.g., light gray/silver background)
- Lower visual priority than selection and persistent marks
- Subtle but visible

**Constraints:**
- Minimum selection length rules:
  - **1 character:** Only highlight if it's NOT a letter (a-z, A-Z)
    - `{`, `$`, `@`, `1`, etc. → highlight all occurrences
    - `a`, `Z`, `m`, etc. → no highlights (too noisy)
  - **2+ characters:** Always highlight
- Maximum occurrences shown: 1000 (performance limit)
- Case-sensitive matching (matches exact selection)

### Technical Design

**No persistent storage needed** - computed on-demand during render.

```rust
/// Transient highlights computed from current selection
pub struct SelectionHighlights {
    /// The selected text to match (empty if no selection or doesn't meet criteria)
    pattern: String,
    /// Cached match positions (byte offsets), recomputed when selection changes
    matches: Vec<Range<usize>>,
    /// Selection generation when matches were computed
    cached_generation: u32,
}

impl SelectionHighlights {
    /// Check if selection should trigger auto-highlight
    fn should_highlight(text: &str) -> bool {
        match text.len() {
            0 => false,
            1 => {
                // Single char: only if NOT a letter
                let ch = text.chars().next().unwrap();
                !ch.is_ascii_alphabetic()
            }
            _ => true, // 2+ chars: always highlight
        }
    }
}
```

**Algorithm:**
1. On render, check if selection exists and meets criteria
2. If selection generation changed, recompute matches:
   - Get selected text
   - Check `should_highlight()` - skip if single letter
   - Search document for all occurrences (using existing search infrastructure or simple byte scan)
   - Cache results
3. Render cached matches as background highlights (excluding the selection itself)

**Integration point:** Computed in `TextBuffer::render()`, rendered before persistent marks.

**Performance:**
- Use SIMD-accelerated search (existing `memchr` infrastructure)
- Limit to first 1000 matches
- Only search visible region + buffer for scrolling
- Cache invalidated only when selection changes

**TODO (Future):** Add debouncing - skip computation if typing rapidly (<100ms between keys).

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Selection is single letter (a-z, A-Z) | No auto-highlights |
| Selection is single non-letter (`{`, `$`, `1`) | Highlight all occurrences |
| Selection is 2+ characters | Highlight all occurrences |
| Selection is whitespace only | No auto-highlights |
| Selection contains newlines | Still matches (multi-line patterns) |
| 10,000+ matches | Limit to 1000, prioritize near cursor |
| Binary/invalid UTF-8 | Match bytes exactly |
| Selection changed by typing | Clear highlights, recompute |

---

## Part 2: Persistent Marks (Ctrl+M)

### Goals

1. **Multiple persistent highlights** - Mark arbitrary text regions that survive navigation and editing
2. **Coexistence with search** - Marks are separate from Ctrl+F search results
3. **Multiple colors** - Support 5 preset highlight colors
4. **Performance** - Handle large files (1GB+) and many marks efficiently
5. **Edit resilience** - Marks adjust when text is inserted/deleted

### Non-Goals (Out of Scope for v1)

- Mark persistence across sessions (save/load marks to disk)
- Regex-based automatic marking
- Mark navigation (jump to next/previous mark)
- Mark in margin/gutter display
- Custom user-defined colors

---

### User Experience

#### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+M` | Mark selection with next color (cycle 1→2→3→4→5→remove) |
| `Ctrl+Shift+M` | Remove all marks in selection |
| `Alt+1` through `Alt+5` | Mark selection with specific color |
| `Ctrl+Alt+M` | Clear ALL marks in document |

**Shortcut Conflict Check:** Alt+1 through Alt+5 are **available** (not used by existing features).

#### Menu Integration

**New top-level "Mark" menu** added after "View":

```
File  Edit  View  Mark  Help
                   │
                   ├── Mark Selection      Ctrl+M
                   ├── ─────────────────────────
                   ├── Mark Yellow         Alt+1
                   ├── Mark Green          Alt+2
                   ├── Mark Cyan           Alt+3
                   ├── Mark Magenta        Alt+4
                   ├── Mark Red            Alt+5
                   ├── ─────────────────────────
                   ├── Clear Marks         Ctrl+Shift+M
                   └── Clear All Marks     Ctrl+Alt+M
```

**Note:** The menu system does NOT support submenus - all items are flat.

#### Visual Appearance

- Marks render as **background color** behind text
- Marks use **50% alpha** blending (semi-transparent)
- Overlapping marks blend colors

**Render order (bottom to top):**
1. Line highlight (if enabled)
2. **Selection auto-highlights** (subtle gray)
3. **Persistent marks** (colored)
4. Selection (blue)
5. Text foreground

**Color Palette (5 colors):**
| Index | Shortcut | Name    | RGB |
|-------|----------|---------|-----|
| 1     | Alt+1    | Yellow  | `#FFFF00` |
| 2     | Alt+2    | Green   | `#00FF00` |
| 3     | Alt+3    | Cyan    | `#00FFFF` |
| 4     | Alt+4    | Magenta | `#FF00FF` |
| 5     | Alt+5    | Red     | `#FF6666` |

---

### Technical Design

#### Data Structures

```rust
/// A single persistent highlight mark
#[derive(Clone, Debug)]
pub struct Mark {
    /// Start byte offset (inclusive)
    pub start: usize,
    /// End byte offset (exclusive)
    pub end: usize,
    /// Color index (1-5)
    pub color: u8,
}

/// Collection of persistent marks for a document
pub struct MarkCollection {
    /// Marks sorted by start offset (ascending)
    marks: Vec<Mark>,
}
```

**Storage:** `MarkCollection` stored in `TextBuffer` struct. Each document has independent marks.

#### Mark Adjustment on Text Edits

When text is modified, all marks must be adjusted.

**On Insert at offset `pos` with length `len`:**
```rust
for mark in &mut marks {
    if mark.start >= pos {
        mark.start += len;
    }
    if mark.end >= pos {
        mark.end += len;
    }
}
```

**On Delete from offset `start` to `end`:**
```rust
let len = end - start;
marks.retain_mut(|mark| {
    if mark.end <= start {
        // Mark entirely before deletion - unchanged
        true
    } else if mark.start >= end {
        // Mark entirely after deletion - shift backward
        mark.start -= len;
        mark.end -= len;
        true
    } else if mark.start >= start && mark.end <= end {
        // Mark entirely within deletion - remove it
        false
    } else if mark.start < start && mark.end > end {
        // Deletion is inside mark - shrink mark
        mark.end -= len;
        true
    } else if mark.start < start {
        // Mark overlaps deletion start - truncate end
        mark.end = start;
        true
    } else {
        // Mark overlaps deletion end - truncate start
        mark.start = start;
        mark.end -= len;
        true
    }
});
// Remove zero-length marks
marks.retain(|mark| mark.start < mark.end);
```

#### Grapheme Boundary Handling

**On mark creation:**
- Snap `start` to nearest grapheme cluster start (round down)
- Snap `end` to nearest grapheme cluster end (round up)

**During rendering:**
- Iterate through visible text character by character
- Check if current byte offset falls within any mark's range
- Apply mark color if within range

#### Rendering Pipeline

**Integration point:** `TextBuffer::render()` in `src/buffer/mod.rs`

```rust
// In render loop, for each character:

// 1. Selection auto-highlight (if not the selection itself)
if self.selection_highlights.match_at_offset(current_offset, selection_range) {
    let bg = SELECTION_HIGHLIGHT_COLOR; // subtle gray
    fb.blend_bg(char_rect, bg);
}

// 2. Persistent marks
if let Some(color) = self.marks.color_at_offset(current_offset) {
    let bg = mark_colors::get(color);
    fb.blend_bg(char_rect, bg);
}

// 3. Selection (existing code)
// ...
```

**Performance optimization:**
```rust
impl MarkCollection {
    /// Find all marks overlapping the byte range [start, end)
    pub fn marks_in_range(&self, start: usize, end: usize) -> &[Mark] {
        let first = self.marks.partition_point(|m| m.end <= start);
        let last = self.marks[first..].partition_point(|m| m.start < end);
        &self.marks[first..first + last]
    }
}
```

---

### Interaction Between Systems

| Feature | Selection Auto-Highlight | Persistent Marks |
|---------|-------------------------|------------------|
| Trigger | Automatic on selection | Manual (Ctrl+M, Alt+1-5) |
| Lifetime | Transient (clears on deselect) | Persistent until cleared |
| Survives edits | N/A (recomputed) | Yes (adjusted) |
| Color | Single (gray) | 5 colors |
| Storage | Computed on-demand | `Vec<Mark>` in buffer |
| Undo/Redo | N/A | Not tracked |

**Visual layering:**
- Auto-highlights render below marks
- Both render below selection
- All three can be visible simultaneously

---

## Implementation Plan

### Phase 1: Selection Auto-Highlight
**Files:** `src/buffer/mod.rs`

1. Add `SelectionHighlights` struct to `TextBuffer`
2. Implement `should_highlight()` logic (single non-letter or 2+ chars)
3. Implement pattern matching using existing search/memchr
4. Integrate into render loop (before selection rendering)
5. Add generation tracking to avoid recomputation

### Phase 2: Persistent Marks - Core
**Files:** `src/buffer/marks.rs` (new), `src/buffer/mod.rs`

1. Create `Mark` and `MarkCollection` structs
2. Add `marks: MarkCollection` field to `TextBuffer`
3. Implement mark adjustment in `edit_write()` and `edit_delete()`
4. Add unit tests for adjustment logic

### Phase 3: Persistent Marks - Rendering
**Files:** `src/buffer/mod.rs`

1. Add `marks_in_range()` query method
2. Integrate mark rendering into `render()` function
3. Add color blending (after auto-highlights, before selection)

### Phase 4: User Interface
**Files:** `src/bin/edit/draw_editor.rs`, `src/bin/edit/state.rs`

1. Add keyboard shortcut handlers (Ctrl+M, Alt+1-5, Ctrl+Shift+M, Ctrl+Alt+M)
2. Implement mark creation/removal functions
3. Handle color cycling for Ctrl+M

### Phase 5: Menu Integration
**Files:** `src/bin/edit/draw_menubar.rs`, `i18n/edit.toml`

1. Add "Mark" menu after "View" in menubar
2. Add menu items (flat structure)
3. Add i18n strings for all menu items
4. Wire menu items to mark functions

---

## Edge Cases and Handling

### Selection Auto-Highlight

| Scenario | Behavior |
|----------|----------|
| Selection is single letter (a-z, A-Z) | No auto-highlights |
| Selection is single non-letter | Highlight all occurrences |
| Selection is 2+ characters | Highlight all occurrences |
| Selection is whitespace only | No auto-highlights |
| 50,000 matches in file | Limit to 1000 nearest cursor |
| Selection changes rapidly (typing) | TODO: Add debouncing |

### Persistent Marks

| Scenario | Behavior |
|----------|----------|
| Insert inside mark | Mark expands |
| Delete inside mark | Mark shrinks |
| Delete entire marked region | Mark removed |
| Ctrl+M with no selection | Do nothing |
| Ctrl+M on already-marked text | Cycle to next color, then remove |
| Mark overlaps existing mark (same color) | Merge |
| Mark overlaps existing mark (different color) | New color overwrites |

### Document Lifecycle

| Scenario | Selection Auto-Highlight | Persistent Marks |
|----------|-------------------------|------------------|
| Close document | N/A | Discarded |
| Reload file (F5) | Recomputed | Cleared |
| Switch documents | Per-document | Per-document |
| Undo/Redo | Recomputed | Remain as-is |

---

## Performance Considerations

### Selection Auto-Highlight
- SIMD search: >100GB/s using existing memchr infrastructure
- Cache matches, invalidate on selection change
- Limit matches to 1000 to cap rendering cost
- TODO: Add debouncing for rapid typing

### Persistent Marks
- Memory: 24 bytes per mark, 10,000 marks = 240 KB
- Adjustment: O(n) per edit, fast for typical mark counts
- Rendering: O(log n) binary search + O(k) visible marks

---

## API Reference

### SelectionHighlights

```rust
impl SelectionHighlights {
    /// Check if selection text should trigger auto-highlight
    pub fn should_highlight(text: &str) -> bool;

    /// Update matches if selection changed
    pub fn update(&mut self, selection_text: Option<&str>, buffer: &GapBuffer, generation: u32);

    /// Check if offset is within a match (excluding primary selection)
    pub fn match_at_offset(&self, offset: usize, selection_range: Range<usize>) -> bool;

    /// Clear all matches
    pub fn clear(&mut self);
}
```

### MarkCollection

```rust
impl MarkCollection {
    pub fn new() -> Self;
    pub fn add(&mut self, start: usize, end: usize, color: u8);
    pub fn remove_range(&mut self, start: usize, end: usize);
    pub fn clear(&mut self);
    pub fn marks_in_range(&self, start: usize, end: usize) -> &[Mark];
    pub fn color_at_offset(&self, offset: usize) -> Option<u8>;
    pub fn adjust_insert(&mut self, pos: usize, len: usize);
    pub fn adjust_delete(&mut self, start: usize, end: usize);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

---

## Color Constants

```rust
pub mod highlight_colors {
    use crate::framebuffer::StraightRgba;

    /// Selection auto-highlight color (subtle gray)
    pub const SELECTION_MATCH: StraightRgba = StraightRgba::new(128, 128, 128, 64);

    /// Persistent mark colors (50% alpha)
    pub const MARK_YELLOW: StraightRgba = StraightRgba::new(255, 255, 0, 128);
    pub const MARK_GREEN: StraightRgba = StraightRgba::new(0, 255, 0, 128);
    pub const MARK_CYAN: StraightRgba = StraightRgba::new(0, 255, 255, 128);
    pub const MARK_MAGENTA: StraightRgba = StraightRgba::new(255, 0, 255, 128);
    pub const MARK_RED: StraightRgba = StraightRgba::new(255, 102, 102, 128);

    pub const MARK_COLORS: [StraightRgba; 5] = [
        MARK_YELLOW, MARK_GREEN, MARK_CYAN, MARK_MAGENTA, MARK_RED
    ];

    pub fn mark_color(index: u8) -> StraightRgba {
        MARK_COLORS.get(index.saturating_sub(1) as usize)
            .copied()
            .unwrap_or(MARK_YELLOW)
    }
}
```

---

## Localization Strings (i18n/edit.toml)

```toml
# Mark menu
[Mark]
en = "Mark"

[MarkSelection]
en = "Mark Selection"

[MarkYellow]
en = "Mark Yellow"

[MarkGreen]
en = "Mark Green"

[MarkCyan]
en = "Mark Cyan"

[MarkMagenta]
en = "Mark Magenta"

[MarkRed]
en = "Mark Red"

[ClearMarks]
en = "Clear Marks"

[ClearAllMarks]
en = "Clear All Marks"
```

---

## Decision Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Two separate systems | Yes | Different use cases, different lifetimes |
| Auto-highlight: single letter | Skip (a-z, A-Z) | Too noisy, not useful |
| Auto-highlight: single non-letter | Highlight | Useful for brackets, symbols |
| Auto-highlight max matches | 1000 | Performance limit |
| Auto-highlight color | Gray | Subtle, doesn't compete with marks |
| Auto-highlight debouncing | TODO | Start simple, add later if needed |
| Menu placement | New "Mark" menu after View | Cleaner organization |
| Menu structure | Flat (no submenus) | Menu system limitation |
| Alt+1-5 shortcuts | Confirmed available | No conflicts |
| Mark colors | 5 fixed | Matches Notepad++, sufficient |
| Mark persistence | Not in v1 | Reduce scope |
| Ctrl+M behavior | Cycle colors then remove | Intuitive toggle |

---

## Future Enhancements (Post-v1)

1. **Auto-highlight debouncing** - Skip computation during rapid typing
2. **Mark persistence** - Save/load marks to `.ogedit/marks/` directory
3. **Mark navigation** - F3/Shift+F3 to jump between marks
4. **Mark panel** - Side panel listing all marks with preview
5. **Mark from search** - "Mark All" button in Find dialog
6. **Custom colors** - User-defined mark colors in settings
7. **Mark bookmarks** - Named marks that can be jumped to
8. **Mark in gutter** - Show mark indicator in line number gutter
