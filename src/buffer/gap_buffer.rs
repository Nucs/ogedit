// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::Range;
use std::ptr::{self, NonNull};
use std::slice;

use crate::document::{ReadableDocument, WriteableDocument};
use crate::helpers::*;
use crate::{apperr, sys};

#[cfg(target_pointer_width = "32")]
const LARGE_CAPACITY: usize = 128 * MEBI;
#[cfg(target_pointer_width = "64")]
const LARGE_CAPACITY: usize = 4 * GIBI;
const LARGE_ALLOC_CHUNK: usize = 64 * KIBI;
const LARGE_GAP_CHUNK: usize = 4 * KIBI;

const SMALL_CAPACITY: usize = 128 * KIBI;
const SMALL_ALLOC_CHUNK: usize = 256;
const SMALL_GAP_CHUNK: usize = 16;

// TODO: Instead of having a specialization for small buffers here,
// tui.rs could also just keep a MRU set of large buffers around.
enum BackingBuffer {
    VirtualMemory(NonNull<u8>, usize),
    Vec(Vec<u8>),
}

impl Drop for BackingBuffer {
    fn drop(&mut self) {
        unsafe {
            if let Self::VirtualMemory(ptr, reserve) = *self {
                sys::virtual_release(ptr, reserve);
            }
        }
    }
}

/// Most people know how `Vec<T>` works: It has some spare capacity at the end,
/// so that pushing into it doesn't reallocate every single time. A gap buffer
/// is the same thing, but the spare capacity can be anywhere in the buffer.
/// This variant is optimized for large buffers and uses virtual memory.
pub struct GapBuffer {
    /// Pointer to the buffer.
    text: NonNull<u8>,
    /// Maximum size of the buffer, including gap.
    reserve: usize,
    /// Size of the buffer, including gap.
    commit: usize,
    /// Length of the stored text, NOT including gap.
    text_length: usize,
    /// Gap offset.
    gap_off: usize,
    /// Gap length.
    gap_len: usize,
    /// Increments every time the buffer is modified.
    generation: u32,
    /// Never rewound by undo: divergent edits must have distinct revisions.
    generation_counter: u32,
    /// If `Vec(..)`, the buffer is optimized for small amounts of text
    /// and uses the standard heap. Otherwise, it uses virtual memory.
    buffer: BackingBuffer,
}

impl GapBuffer {
    pub fn new(small: bool) -> apperr::Result<Self> {
        let reserve;
        let buffer;
        let text;

        if small {
            reserve = SMALL_CAPACITY;
            text = NonNull::dangling();
            buffer = BackingBuffer::Vec(Vec::new());
        } else {
            reserve = LARGE_CAPACITY;
            text = unsafe { sys::virtual_reserve(reserve)? };
            buffer = BackingBuffer::VirtualMemory(text, reserve);
        }

        Ok(Self {
            text,
            reserve,
            commit: 0,
            text_length: 0,
            gap_off: 0,
            gap_len: 0,
            generation: 0,
            generation_counter: 0,
            buffer,
        })
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.text_length
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn set_generation(&mut self, generation: u32) {
        self.generation = generation;
    }

    pub fn bump_generation(&mut self) {
        self.generation_counter = self.generation_counter.wrapping_add(1);
        self.generation = self.generation_counter;
    }

    /// Continue the revision sequence when replacing a fully loaded document.
    pub(super) fn advance_revision_from(&mut self, previous: &Self) {
        self.generation_counter = previous.generation_counter;
        self.bump_generation();
    }

    /// Reserve a larger address range before loading a large file. No pages are
    /// committed, and an error leaves the existing allocation intact.
    pub fn reserve_empty(&mut self, bytes: usize) -> apperr::Result<()> {
        if bytes <= self.reserve {
            return Ok(());
        }
        if self.text_length != 0 || !matches!(self.buffer, BackingBuffer::VirtualMemory(..)) {
            return Err(std::io::Error::from(std::io::ErrorKind::OutOfMemory).into());
        }
        let bytes = bytes
            .checked_add(MEBI + LARGE_ALLOC_CHUNK - 1)
            .map(|n| n & !(LARGE_ALLOC_CHUNK - 1))
            .filter(|&n| n <= isize::MAX as usize)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
        let ptr = unsafe { sys::virtual_reserve(bytes)? };
        // Assigning drops the previous backing once; never release it manually.
        self.buffer = BackingBuffer::VirtualMemory(ptr, bytes);
        self.text = ptr;
        self.reserve = bytes;
        self.commit = 0;
        self.gap_off = 0;
        self.gap_len = 0;
        Ok(())
    }

    /// Reserve insertion space without changing document contents or generation.
    /// A failed reservation leaves the document intact.
    pub fn reserve_insert(&mut self, len: usize) -> bool {
        if len <= self.gap_len {
            return true;
        }
        if len > self.reserve.saturating_sub(self.text_length) {
            return false;
        }
        self.enlarge_gap(len);
        len <= self.gap_len
    }

    /// WARNING: The returned slice must not necessarily be the same length as `len` (due to OOM).
    pub fn allocate_gap(&mut self, off: usize, len: usize, delete: usize) -> &mut [u8] {
        // Sanitize parameters
        let off = off.min(self.text_length);
        let delete = delete.min(self.text_length - off);

        // Move the existing gap if it exists
        if off != self.gap_off {
            self.move_gap(off);
        }

        // Delete the text
        if delete > 0 {
            self.delete_text(delete);
        }

        // Enlarge the gap if needed
        if len > self.gap_len {
            self.enlarge_gap(len);
        }

        self.bump_generation();
        unsafe { slice::from_raw_parts_mut(self.text.add(self.gap_off).as_ptr(), self.gap_len) }
    }

    fn move_gap(&mut self, off: usize) {
        if self.gap_len > 0 {
            //
            //                       v gap_off
            // left:  |ABCDEFGHIJKLMN   OPQRSTUVWXYZ|
            //        |ABCDEFGHI   JKLMNOPQRSTUVWXYZ|
            //                  ^ off
            //        move: JKLMN
            //
            //                       v gap_off
            // !left: |ABCDEFGHIJKLMN   OPQRSTUVWXYZ|
            //        |ABCDEFGHIJKLMNOPQRS   TUVWXYZ|
            //                            ^ off
            //        move: OPQRS
            //
            let left = off < self.gap_off;
            let move_src = if left { off } else { self.gap_off + self.gap_len };
            let move_dst = if left { off + self.gap_len } else { self.gap_off };
            let move_len = if left { self.gap_off - off } else { off - self.gap_off };

            unsafe { self.text.add(move_src).copy_to(self.text.add(move_dst), move_len) };

            if cfg!(debug_assertions) {
                // Fill the moved-out bytes with 0xCD to make debugging easier.
                unsafe { self.text.add(off).write_bytes(0xCD, self.gap_len) };
            }
        }

        self.gap_off = off;
    }

    fn delete_text(&mut self, delete: usize) {
        if cfg!(debug_assertions) {
            // Fill the deleted bytes with 0xCD to make debugging easier.
            unsafe { self.text.add(self.gap_off + self.gap_len).write_bytes(0xCD, delete) };
        }

        self.gap_len += delete;
        self.text_length -= delete;
    }

    fn enlarge_gap(&mut self, len: usize) {
        let gap_chunk;
        let alloc_chunk;

        if matches!(self.buffer, BackingBuffer::VirtualMemory(..)) {
            gap_chunk = LARGE_GAP_CHUNK;
            alloc_chunk = LARGE_ALLOC_CHUNK;
        } else {
            gap_chunk = SMALL_GAP_CHUNK;
            alloc_chunk = SMALL_ALLOC_CHUNK;
        }

        let gap_len_old = self.gap_len;
        let gap_len_new = (len.saturating_add(gap_chunk + gap_chunk - 1) & !(gap_chunk - 1))
            .min(self.reserve - self.text_length);

        let bytes_old = self.commit;
        let bytes_new = self.text_length + gap_len_new;

        if bytes_new > bytes_old {
            let bytes_new = (bytes_new + alloc_chunk - 1) & !(alloc_chunk - 1);

            if bytes_new > self.reserve {
                return;
            }

            match &mut self.buffer {
                BackingBuffer::VirtualMemory(ptr, _) => unsafe {
                    if sys::virtual_commit(ptr.add(bytes_old), bytes_new - bytes_old).is_err() {
                        return;
                    }
                },
                BackingBuffer::Vec(v) => {
                    if v.try_reserve_exact(bytes_new - v.len()).is_err() {
                        return;
                    }
                    v.resize(bytes_new, 0);
                    self.text = unsafe { NonNull::new_unchecked(v.as_mut_ptr()) };
                }
            }

            self.commit = bytes_new;
        }

        let gap_beg = unsafe { self.text.add(self.gap_off) };
        unsafe {
            ptr::copy(
                gap_beg.add(gap_len_old).as_ptr(),
                gap_beg.add(gap_len_new).as_ptr(),
                self.text_length - self.gap_off,
            )
        };

        if cfg!(debug_assertions) {
            // Fill the moved-out bytes with 0xCD to make debugging easier.
            unsafe { gap_beg.add(gap_len_old).write_bytes(0xCD, gap_len_new - gap_len_old) };
        }

        self.gap_len = gap_len_new;
    }

    pub fn commit_gap(&mut self, len: usize) {
        assert!(len <= self.gap_len);
        self.text_length += len;
        self.gap_off += len;
        self.gap_len -= len;
    }

    pub fn replace(&mut self, range: Range<usize>, src: &[u8]) {
        // Never delete an existing selection and then discover that its replacement
        // does not fit. Reserve first; callers can explicitly preflight larger batches.
        let start = range.start.min(self.text_length);
        let delete = range.end.min(self.text_length).saturating_sub(start);
        if !self.reserve_insert(src.len().saturating_sub(delete)) {
            return;
        }
        let gap = self.allocate_gap(range.start, src.len(), range.end.saturating_sub(range.start));
        let len = slice_copy_safe(gap, src);
        self.commit_gap(len);
    }

    pub fn clear(&mut self) {
        self.gap_off = 0;
        self.gap_len += self.text_length;
        self.bump_generation();
        self.text_length = 0;
    }

    pub fn extract_raw(&self, range: Range<usize>, out: &mut Vec<u8>, mut out_off: usize) {
        let end = range.end.min(self.text_length);
        let mut beg = range.start.min(end);
        out_off = out_off.min(out.len());

        if beg >= end {
            return;
        }

        out.reserve(end - beg);

        while beg < end {
            let chunk = self.read_forward(beg);
            let chunk = &chunk[..chunk.len().min(end - beg)];
            out.replace_range(out_off..out_off, chunk);
            beg += chunk.len();
            out_off += chunk.len();
        }
    }

    /// Replaces the entire buffer contents with the given `text`.
    /// The method is optimized for the case where the given `text` already matches
    /// the existing contents. Returns `true` if the buffer contents were changed.
    pub fn copy_from(&mut self, src: &dyn ReadableDocument) -> bool {
        let mut off = 0;

        // Find the position at which the contents change.
        loop {
            let dst_chunk = self.read_forward(off);
            let src_chunk = src.read_forward(off);

            let dst_len = dst_chunk.len();
            let src_len = src_chunk.len();
            let len = dst_len.min(src_len);
            let mismatch = dst_chunk[..len] != src_chunk[..len];

            if mismatch {
                break; // The contents differ.
            }
            if len == 0 {
                if dst_len == src_len {
                    return false; // Both done simultaneously. -> Done.
                }
                break; // One of the two is shorter.
            }

            off += len;
        }

        // Update the buffer starting at `off`.
        loop {
            let chunk = src.read_forward(off);
            self.replace(off..usize::MAX, chunk);
            off += chunk.len();

            // No more data to copy -> Done. By checking this _after_ the replace()
            // call, we ensure that the initial `off..usize::MAX` range is deleted.
            // This fixes going from some buffer contents to being empty.
            if chunk.is_empty() {
                return true;
            }
        }
    }

    /// Copies the contents of the buffer into a string.
    pub fn copy_into(&self, dst: &mut dyn WriteableDocument) {
        let mut beg = 0;
        let mut off = 0;

        while {
            let chunk = self.read_forward(off);

            // The first write will be 0..usize::MAX and effectively clear() the destination.
            // Every subsequent write will be usize::MAX..usize::MAX and thus effectively append().
            dst.replace(beg..usize::MAX, chunk);
            beg = usize::MAX;

            off += chunk.len();
            off < self.text_length
        } {}
    }
}

impl ReadableDocument for GapBuffer {
    fn read_forward(&self, off: usize) -> &[u8] {
        let off = off.min(self.text_length);
        let beg;
        let len;

        if off < self.gap_off {
            // Cursor is before the gap: We can read until the start of the gap.
            beg = off;
            len = self.gap_off - off;
        } else {
            // Cursor is after the gap: We can read until the end of the buffer.
            beg = off + self.gap_len;
            len = self.text_length - off;
        }

        unsafe { slice::from_raw_parts(self.text.add(beg).as_ptr(), len) }
    }

    fn read_backward(&self, off: usize) -> &[u8] {
        let off = off.min(self.text_length);
        let beg;
        let len;

        if off <= self.gap_off {
            // Cursor is before the gap: We can read until the beginning of the buffer.
            beg = 0;
            len = off;
        } else {
            // Cursor is after the gap: We can read until the end of the gap.
            beg = self.gap_off + self.gap_len;
            // The cursor_off doesn't account of the gap_len.
            // (This allows us to move the gap without recalculating the cursor position.)
            len = off - self.gap_off;
        }

        unsafe { slice::from_raw_parts(self.text.add(beg).as_ptr(), len) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewinding_revision_does_not_reuse_the_next_edit_identity() {
        let mut buffer = GapBuffer::new(true).unwrap();
        buffer.replace(0..0, b"a");
        let saved = buffer.generation();
        buffer.clear();
        buffer.set_generation(0);
        buffer.replace(0..0, b"b");
        assert_ne!(buffer.generation(), saved);
        assert_contents(&buffer, b"b");
    }

    #[test]
    fn oversized_reservation_is_checked_and_preserves_content() {
        let mut buffer = GapBuffer::new(false).unwrap();
        assert!(buffer.reserve_empty(usize::MAX).is_err());
        assert_contents(&buffer, b"");
        buffer.replace(0..0, b"keep");
        assert!(buffer.reserve_empty(usize::MAX).is_err());
        assert_contents(&buffer, b"keep");
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn large_file_address_reservation_can_exceed_four_gib_without_committing_it() {
        let mut buffer = GapBuffer::new(false).unwrap();
        buffer.reserve_empty(4 * GIBI + MEBI).unwrap();
        assert!(buffer.reserve > 4 * GIBI);
        assert_eq!(buffer.commit, 0);
        buffer.replace(0..0, b"small committed prefix");
        assert!(buffer.commit <= LARGE_ALLOC_CHUNK);
        assert_contents(&buffer, b"small committed prefix");
    }

    fn assert_contents(buffer: &GapBuffer, expected: &[u8]) {
        assert_eq!(buffer.len(), expected.len());
        assert!(buffer.gap_off <= buffer.text_length);
        assert!(buffer.text_length + buffer.gap_len <= buffer.commit);
        assert!(buffer.commit <= buffer.reserve);
        let mut actual = Vec::new();
        buffer.extract_raw(0..usize::MAX, &mut actual, 0);
        assert_eq!(actual, expected);
        // Read in both directions across the gap and at clamped offsets.
        for off in [0, expected.len() / 2, expected.len(), usize::MAX] {
            let clamped = off.min(expected.len());
            let forward = buffer.read_forward(off);
            let backward = buffer.read_backward(off);
            assert_eq!(forward, &expected[clamped..clamped + forward.len()]);
            assert_eq!(backward, &expected[clamped - backward.len()..clamped]);
        }
    }

    #[test]
    fn oversized_gap_request_cannot_wrap_or_corrupt_existing_text() {
        let mut buffer = GapBuffer::new(true).unwrap();
        buffer.replace(0..0, b"left-right");
        assert_eq!(buffer.allocate_gap(5, usize::MAX, 0).len(), SMALL_CAPACITY - 10);
        assert_contents(&buffer, b"left-right");
        let generation = buffer.generation();
        assert!(!buffer.reserve_insert(usize::MAX));
        assert_eq!(buffer.generation(), generation);
        assert_contents(&buffer, b"left-right");
    }

    #[test]
    fn deterministic_random_edits_match_a_vec_for_heap_and_virtual_memory() {
        for small in [true, false] {
            let mut buffer = GapBuffer::new(small).unwrap();
            let mut oracle = Vec::new();
            let mut seed = 0x0bad_5eed_u32;
            let mut random = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as usize
            };
            for iteration in 0..25_000 {
                let start = random() % (oracle.len() + 17);
                let end = if iteration % 23 == 0 {
                    usize::MAX
                } else {
                    start.saturating_add(random() % 97)
                };
                let bytes: Vec<u8> = (0..random() % 97).map(|_| random() as u8).collect();
                buffer.replace(start..end, &bytes);
                let start = start.min(oracle.len());
                let end = end.min(oracle.len()).max(start);
                oracle.splice(start..end, bytes);
                if iteration % 137 == 0 {
                    // Moving/enlarging the gap does not change committed text.
                    let off = random() % (oracle.len() + 1);
                    buffer.allocate_gap(off, random() % 4096, 0);
                }
                assert_contents(&buffer, &oracle);
                if iteration % 997 == 0 {
                    buffer.clear();
                    oracle.clear();
                    assert_contents(&buffer, &oracle);
                }
            }
        }
    }

    #[test]
    fn capacity_failure_preserves_selection_and_neighbors() {
        let mut buffer = GapBuffer::new(true).unwrap();
        let original = vec![0xa5; SMALL_CAPACITY];
        buffer.replace(0..0, &original);
        assert_contents(&buffer, &original);
        let generation = buffer.generation();
        buffer.replace(100..101, &[1, 2]);
        assert_eq!(buffer.generation(), generation);
        assert_contents(&buffer, &original);
        buffer.replace(100..102, &[1, 2]);
        let mut expected = original;
        expected[100..102].copy_from_slice(&[1, 2]);
        assert_contents(&buffer, &expected);
    }
}
