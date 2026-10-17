// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::mut_from_ref)]

use std::alloc::{AllocError, Allocator, Layout};
use std::cell::Cell;
use std::hint::cold_path;
use std::mem::MaybeUninit;
use std::num::NonZeroUsize;
use std::ptr::{self, NonNull};
use std::{mem, slice};

use crate::helpers::*;
use crate::{apperr, sys};

#[cfg(target_pointer_width = "32")]
const ALLOC_CHUNK_SIZE: usize = 32 * KIBI;
#[cfg(target_pointer_width = "64")]
const ALLOC_CHUNK_SIZE: usize = 64 * KIBI;

/// An arena allocator.
///
/// If you have never used an arena allocator before, think of it as
/// allocating objects on the stack, but the stack is *really* big.
/// Each time you allocate, memory gets pushed at the end of the stack,
/// each time you deallocate, memory gets popped from the end of the stack.
///
/// One reason you'd want to use this is obviously performance: It's very simple
/// and so it's also very fast, >10x faster than your system allocator.
///
/// However, modern allocators such as `mimalloc` are just as fast, so why not use them?
/// Because their performance comes at the cost of binary size and we can't have that.
///
/// The biggest benefit though is that it sometimes massively simplifies lifetime
/// and memory management. This can best be seen by this project's UI code, which
/// uses an arena to allocate a tree of UI nodes. This is infamously difficult
/// to do in Rust, but not so when you got an arena allocator:
/// All nodes have the same lifetime, so you can just use references.
///
/// <div class="warning">
///
/// **Do not** push objects into the arena that require destructors.
/// Destructors are not executed. Use a pool allocator for that.
///
/// </div>
pub struct Arena {
    base: NonNull<u8>,
    capacity: usize,
    commit: Cell<usize>,
    offset: Cell<usize>,

    /// Scratch scopes must remain properly nested in every build.
    pub(super) borrows: Cell<usize>,
}

impl Arena {
    pub const fn empty() -> Self {
        Self {
            base: NonNull::dangling(),
            capacity: 0,
            commit: Cell::new(0),
            offset: Cell::new(0),

            borrows: Cell::new(0),
        }
    }

    pub fn new(capacity: usize) -> apperr::Result<Self> {
        let capacity = capacity
            .max(1)
            .checked_add(ALLOC_CHUNK_SIZE - 1)
            .map(|n| n & !(ALLOC_CHUNK_SIZE - 1))
            .filter(|&n| n <= isize::MAX as usize)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
        let base = unsafe { sys::virtual_reserve(capacity)? };

        Ok(Self {
            base,
            capacity,
            commit: Cell::new(0),
            offset: Cell::new(0),

            borrows: Cell::new(0),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.base == NonNull::dangling()
    }

    pub fn offset(&self) -> usize {
        self.offset.get()
    }

    /// "Deallocates" the memory in the arena down to the given offset.
    ///
    /// # Safety
    ///
    /// Obviously, this is GIGA UNSAFE. It runs no destructors and does not check
    /// whether the offset is valid. You better take care when using this function.
    pub unsafe fn reset(&self, to: usize) {
        // Fill the deallocated memory with 0xDD to aid debugging.
        if cfg!(debug_assertions) && self.offset.get() > to {
            let commit = self.commit.get();
            let len = self.offset.get().saturating_add(128).min(commit) - to;
            unsafe { slice::from_raw_parts_mut(self.base.add(to).as_ptr(), len).fill(0xDD) };
        }

        self.offset.replace(to);
    }

    #[inline]
    pub(super) fn alloc_raw(
        &self,
        bytes: usize,
        alignment: usize,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let commit = self.commit.get();
        let offset = self.offset.get();

        if !alignment.is_power_of_two() || bytes > isize::MAX as usize {
            return Err(AllocError);
        }
        if bytes == 0 {
            let ptr = NonNull::without_provenance(NonZeroUsize::new(alignment).unwrap());
            return Ok(NonNull::slice_from_raw_parts(ptr, 0));
        }

        // Virtual memory is page-aligned, not necessarily aligned to the layout.
        // Round the actual address, then check every offset before pointer arithmetic.
        let base = self.base.as_ptr() as usize;
        let address = base.checked_add(offset).ok_or(AllocError)?;
        let aligned = address.checked_add(alignment - 1).ok_or(AllocError)? & !(alignment - 1);
        let beg = aligned.checked_sub(base).ok_or(AllocError)?;
        let end = beg.checked_add(bytes).filter(|&end| end <= self.capacity).ok_or(AllocError)?;

        if end > commit {
            return self.alloc_raw_bump(beg, end);
        }

        if cfg!(debug_assertions) {
            let ptr = unsafe { self.base.add(offset) };
            let len = end.saturating_add(128).min(self.commit.get()) - offset;
            unsafe { slice::from_raw_parts_mut(ptr.as_ptr(), len).fill(0xCD) };
        }

        self.offset.replace(end);
        Ok(unsafe { NonNull::slice_from_raw_parts(self.base.add(beg), bytes) })
    }

    // With the code in `alloc_raw_bump()` out of the way, `alloc_raw()` compiles down to some super tight assembly.
    #[cold]
    fn alloc_raw_bump(&self, beg: usize, end: usize) -> Result<NonNull<[u8]>, AllocError> {
        let offset = self.offset.get();
        let commit_old = self.commit.get();
        let commit_new =
            end.checked_add(ALLOC_CHUNK_SIZE - 1).ok_or(AllocError)? & !(ALLOC_CHUNK_SIZE - 1);

        if commit_new > self.capacity
            || unsafe {
                sys::virtual_commit(self.base.add(commit_old), commit_new - commit_old).is_err()
            }
        {
            return Err(AllocError);
        }

        if cfg!(debug_assertions) {
            let ptr = unsafe { self.base.add(offset) };
            let len = end.saturating_add(128).min(commit_new) - offset;
            unsafe { slice::from_raw_parts_mut(ptr.as_ptr(), len).fill(0xCD) };
        }

        self.commit.replace(commit_new);
        self.offset.replace(end);
        Ok(unsafe { NonNull::slice_from_raw_parts(self.base.add(beg), end - beg) })
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_uninit<T>(&self) -> &mut MaybeUninit<T> {
        let bytes = mem::size_of::<T>();
        let alignment = mem::align_of::<T>();
        let ptr = self.alloc_raw(bytes, alignment).unwrap();
        unsafe { ptr.cast().as_mut() }
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_uninit_slice<T>(&self, count: usize) -> &mut [MaybeUninit<T>] {
        let layout = Layout::array::<T>(count).expect("arena allocation size overflow");
        let ptr = self.alloc_raw(layout.size(), layout.align()).unwrap();
        unsafe { slice::from_raw_parts_mut(ptr.cast().as_ptr(), count) }
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        if !self.is_empty() {
            unsafe { sys::virtual_release(self.base, self.capacity) };
        }
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::empty()
    }
}

unsafe impl Allocator for Arena {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_raw(layout.size(), layout.align())
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let p = self.alloc_raw(layout.size(), layout.align())?;
        unsafe { p.cast::<u8>().as_ptr().write_bytes(0, p.len()) }
        Ok(p)
    }

    // While it is possible to shrink the tail end of the arena, it is
    // not very useful given the existence of scoped scratch arenas.
    unsafe fn deallocate(&self, _: NonNull<u8>, _: Layout) {}

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        debug_assert!(new_layout.size() >= old_layout.size());

        let new_ptr;

        // Growing the given area is possible if it is at the end of the arena.
        if (ptr.as_ptr() as usize).is_multiple_of(new_layout.align())
            && unsafe { ptr.add(old_layout.size()) == self.base.add(self.offset.get()) }
        {
            new_ptr = ptr;
            let delta = new_layout.size() - old_layout.size();
            // Assuming that the given ptr/length area is at the end of the arena,
            // we can just push more memory to the end of the arena to grow it.
            self.alloc_raw(delta, 1)?;
        } else {
            cold_path();

            new_ptr = self.allocate(new_layout)?.cast();

            // SAFETY: grow's caller guarantees that the new layout is at least as large.
            unsafe {
                if old_layout.size() != 0 {
                    ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_ptr(), old_layout.size());
                }
                self.deallocate(ptr, old_layout);
            }
        }

        Ok(NonNull::slice_from_raw_parts(new_ptr, new_layout.size()))
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        unsafe {
            // SAFETY: Same as grow().
            let ptr = self.grow(ptr, old_layout, new_layout)?;

            // SAFETY: At this point, `ptr` must be valid for `new_layout.size()` bytes,
            // allowing us to safely zero out the delta since `old_layout.size()`.
            ptr.cast::<u8>()
                .add(old_layout.size())
                .write_bytes(0, new_layout.size() - old_layout.size());

            Ok(ptr)
        }
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        debug_assert!(new_layout.size() <= old_layout.size());

        if !(ptr.as_ptr() as usize).is_multiple_of(new_layout.align()) {
            let new_ptr = self.allocate(new_layout)?;
            // SAFETY: shrink's caller guarantees both regions hold new_layout.size() bytes.
            unsafe {
                ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.cast().as_ptr(), new_layout.size());
                self.deallocate(ptr, old_layout);
            }
            return Ok(new_ptr);
        }

        let mut len = old_layout.size();

        // Shrinking the given area is possible if it is at the end of the arena.
        if unsafe { ptr.add(len) == self.base.add(self.offset.get()) } {
            self.offset.set(self.offset.get() - len + new_layout.size());
            len = new_layout.size();
        }

        Ok(NonNull::slice_from_raw_parts(ptr, len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impossible_sizes_fail_without_changing_the_watermark() {
        assert!(Arena::new(usize::MAX).is_err());
        assert!(Arena::new(isize::MAX as usize).is_err());
        let arena = Arena::new(ALLOC_CHUNK_SIZE).unwrap();
        let first = arena.allocate(Layout::from_size_align(16, 8).unwrap()).unwrap();
        unsafe { first.cast::<u8>().write_bytes(0x5a, 16) };
        let offset = arena.offset();
        assert!(arena.alloc_raw(usize::MAX, 8).is_err());
        assert!(arena.alloc_raw(ALLOC_CHUNK_SIZE, 8).is_err());
        assert_eq!(arena.offset(), offset);
        assert!(unsafe { first.as_ref() }.iter().all(|&b| b == 0x5a));
        let overflow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arena.alloc_uninit_slice::<u64>(usize::MAX / 8 + 1);
        }));
        assert!(overflow.is_err());
        assert_eq!(arena.offset(), offset);
    }

    #[test]
    fn zero_sized_allocations_are_aligned_even_in_an_empty_arena() {
        let arena = Arena::empty();
        for align in [1, 8, 4096, 1 << 29] {
            let ptr = arena.allocate(Layout::from_size_align(0, align).unwrap()).unwrap();
            assert_eq!(ptr.len(), 0);
            assert_eq!(ptr.cast::<u8>().as_ptr() as usize % align, 0);
            assert_eq!(arena.offset(), 0);
        }
        #[repr(align(4096))]
        struct AlignedZst;
        assert_eq!(arena.alloc_uninit::<AlignedZst>() as *mut _ as usize % 4096, 0);
    }

    #[test]
    fn alignment_uses_the_base_address_and_grow_shrink_can_raise_it() {
        let owner = Arena::new(2 * ALLOC_CHUNK_SIZE).unwrap();
        owner.alloc_raw(2 * ALLOC_CHUNK_SIZE, 1).unwrap();
        // A deliberately shifted base reproduces page alignment being weaker
        // than a requested type alignment, regardless of the OS address hint.
        let arena = mem::ManuallyDrop::new(Arena {
            base: unsafe { owner.base.add(1) },
            capacity: owner.capacity - 1,
            commit: Cell::new(owner.capacity - 1),
            offset: Cell::new(0),
            borrows: Cell::new(0),
        });
        let aligned = arena.allocate(Layout::from_size_align(8, 4096).unwrap()).unwrap();
        assert_eq!(aligned.cast::<u8>().as_ptr() as usize % 4096, 0);

        // Force an odd address, then require a greater alignment while growing.
        arena.alloc_raw(1, 1).unwrap();
        let old_layout = Layout::from_size_align(8, 1).unwrap();
        let old = arena.allocate(old_layout).unwrap();
        unsafe { old.cast::<u8>().write_bytes(0x6b, 8) };
        let grown_layout = Layout::from_size_align(32, 64).unwrap();
        let grown = unsafe { arena.grow_zeroed(old.cast(), old_layout, grown_layout).unwrap() };
        assert_eq!(grown.cast::<u8>().as_ptr() as usize % 64, 0);
        assert_eq!(&unsafe { grown.as_ref() }[..8], &[0x6b; 8]);
        assert_eq!(&unsafe { grown.as_ref() }[8..], &[0; 24]);

        // Shrinking may increase alignment too; the preserved prefix must survive.
        let shrunk_layout = Layout::from_size_align(4, 8192).unwrap();
        let shrunk = unsafe { arena.shrink(grown.cast(), grown_layout, shrunk_layout).unwrap() };
        assert_eq!(shrunk.cast::<u8>().as_ptr() as usize % 8192, 0);
        assert_eq!(&unsafe { shrunk.as_ref() }[..4], &[0x6b; 4]);
    }

    #[test]
    fn repeated_mixed_layout_allocations_preserve_live_neighbors() {
        let arena = Arena::new(2 * MEBI).unwrap();
        let mut seed = 0x7c39_52e1u32;
        for _ in 0..128 {
            let mut live = Vec::new();
            for _ in 0..128 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let len = seed as usize % 513;
                let align = 1 << (seed as usize % 13);
                let byte = (seed >> 16) as u8;
                let ptr =
                    arena.allocate_zeroed(Layout::from_size_align(len, align).unwrap()).unwrap();
                assert_eq!(ptr.cast::<u8>().as_ptr() as usize % align, 0);
                assert!(unsafe { ptr.as_ref() }.iter().all(|&b| b == 0));
                unsafe { ptr.cast::<u8>().write_bytes(byte, len) };
                live.push((ptr, byte));
            }
            for (ptr, byte) in live {
                assert!(unsafe { ptr.as_ref() }.iter().all(|&b| b == byte));
            }
            // All allocations are dead before reusing the arena.
            unsafe { arena.reset(0) };
            assert_eq!(arena.offset(), 0);
        }
    }
}
