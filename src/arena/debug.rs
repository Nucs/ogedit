// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::missing_safety_doc, clippy::mut_from_ref)]

use std::alloc::{AllocError, Allocator, Layout};
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::rc::Rc;

use super::release;
use crate::apperr;

/// A borrow-checking wrapper for [`release::Arena`], enabled in every build.
///
/// The problem with [`super::ScratchArena`] is that it only "borrows" an underlying
/// [`release::Arena`]. Once the [`super::ScratchArena`] is dropped it resets the watermark
/// of the underlying [`release::Arena`], freeing all allocations done since borrowing it.
///
/// It is completely valid for the same [`release::Arena`] to be borrowed multiple times at once,
/// *as long as* you only use the most recent borrow. Bad example:
/// ```should_panic
/// use ogedit::arena::scratch_arena;
///
/// let mut scratch1 = scratch_arena(None);
/// let mut scratch2 = scratch_arena(None);
///
/// let foo = scratch1.alloc_uninit::<usize>();
///
/// // This will also reset `scratch1`'s allocation.
/// drop(scratch2);
///
/// *foo; // The checked allocator prevents this invalid access in every build.
/// ```
///
/// Each scope gets a distinct checked wrapper around the shared allocator.
/// Keeping the delegate private prevents callers from bypassing its scope checks.
pub struct Arena {
    kind: ArenaKind,
}

enum ArenaKind {
    // A scope owns its backing allocator even during TLS teardown. Public
    // references remain tied to &self, rather than the underlying Rc lifetime.
    Delegated { delegate: Rc<release::Arena>, borrow: usize },
    Owned { arena: release::Arena },
}

impl Drop for Arena {
    fn drop(&mut self) {
        if let ArenaKind::Delegated { delegate, borrow } = &self.kind {
            let borrows = delegate.borrows.get();
            assert_eq!(*borrow, borrows);
            delegate.borrows.set(borrows - 1);
        }
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::empty()
    }
}

impl Arena {
    pub const fn empty() -> Self {
        Self { kind: ArenaKind::Owned { arena: release::Arena::empty() } }
    }

    pub fn new(capacity: usize) -> apperr::Result<Self> {
        Ok(Self { kind: ArenaKind::Owned { arena: release::Arena::new(capacity)? } })
    }

    pub(super) fn delegated(delegate: Rc<release::Arena>) -> Self {
        let borrow = delegate.borrows.get().checked_add(1).expect("too many scratch scopes");
        delegate.borrows.set(borrow);
        Self { kind: ArenaKind::Delegated { delegate, borrow } }
    }

    #[inline]
    pub(super) fn delegate_target(&self) -> &release::Arena {
        match &self.kind {
            ArenaKind::Delegated { delegate, borrow } => {
                assert!(
                    *borrow == delegate.borrows.get(),
                    "Arena already borrowed by a newer ScratchArena"
                );
                delegate
            }
            ArenaKind::Owned { arena } => arena,
        }
    }

    #[inline]
    pub(super) fn delegate_target_unchecked(&self) -> &release::Arena {
        match &self.kind {
            ArenaKind::Delegated { delegate, .. } => delegate,
            ArenaKind::Owned { arena } => arena,
        }
    }

    pub fn offset(&self) -> usize {
        self.delegate_target().offset()
    }

    pub unsafe fn reset(&self, to: usize) {
        unsafe { self.delegate_target().reset(to) }
    }

    pub fn alloc_uninit<T>(&self) -> &mut MaybeUninit<T> {
        self.delegate_target().alloc_uninit()
    }

    pub fn alloc_uninit_slice<T>(&self, count: usize) -> &mut [MaybeUninit<T>] {
        self.delegate_target().alloc_uninit_slice(count)
    }
}

unsafe impl Allocator for Arena {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.delegate_target().alloc_raw(layout.size(), layout.align())
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.delegate_target().allocate_zeroed(layout)
    }

    // While it is possible to shrink the tail end of the arena, it is
    // not very useful given the existence of scoped scratch arenas.
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { self.delegate_target().deallocate(ptr, layout) }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { self.delegate_target().grow(ptr, old_layout, new_layout) }
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { self.delegate_target().grow_zeroed(ptr, old_layout, new_layout) }
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { self.delegate_target().shrink(ptr, old_layout, new_layout) }
    }
}
