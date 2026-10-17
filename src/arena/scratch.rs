// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::ops::Deref;
use std::ptr;
use std::rc::Rc;

use super::{Arena, release};
use crate::apperr;
use crate::helpers::*;

/// Borrows an [`Arena`] for temporary allocations.
///
/// Scopes must be used and dropped in reverse construction order. This is
/// checked in every build before allocating or resetting their shared arena.
/// See [`scratch_arena`].
pub struct ScratchArena<'a> {
    arena: Arena,
    offset: usize,
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a> ScratchArena<'a> {
    fn new(arena: Rc<release::Arena>) -> Self {
        let offset = arena.offset();
        ScratchArena { arena: Arena::delegated(arena), _phantom: std::marker::PhantomData, offset }
    }
}

impl Drop for ScratchArena<'_> {
    fn drop(&mut self) {
        unsafe { self.arena.reset(self.offset) };
    }
}

impl Deref for ScratchArena<'_> {
    type Target = Arena;

    fn deref(&self) -> &Self::Target {
        &self.arena
    }
}

thread_local! {
    // The application now has background workers. A process-global scratch
    // pool was only safe when every caller happened to run on the UI thread.
    static S_SCRATCH: [RefCell<Option<Rc<release::Arena>>>; 2] =
        const { [RefCell::new(None), RefCell::new(None)] };
}

/// Initialize this thread's scratch arenas with the given capacity.
/// Existing live scopes prevent reinitialization, so references cannot be invalidated.
pub fn init(capacity: usize) -> apperr::Result<()> {
    S_SCRATCH.with(|pool| {
        if pool.iter().any(|s| s.borrow().as_ref().is_some_and(|arena| arena.borrows.get() != 0)) {
            return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock).into());
        }
        // Construct both before replacing either one, including on allocation failure.
        let replacements =
            [Rc::new(release::Arena::new(capacity)?), Rc::new(release::Arena::new(capacity)?)];
        for (slot, arena) in pool.iter().zip(replacements) {
            slot.replace(Some(arena));
        }
        Ok(())
    })
}

/// Returns a thread-local arena that resets when the scope is dropped.
///
/// Pass any arena holding the caller's output as `conflict`; temporary
/// allocations will use the other arena. Nested scopes on one arena must be
/// used and dropped in reverse construction order. Incorrect nesting is checked
/// in release builds too, before memory can be reused under a live reference.
///
/// Each worker thread initializes its pool lazily and releases it on thread exit.
/// Scratch scopes are not `Send` or `Sync` and cannot cross thread boundaries.
pub fn scratch_arena(conflict: Option<&Arena>) -> ScratchArena<'static> {
    let conflict = conflict.map(|a| a.delegate_target_unchecked());
    S_SCRATCH.with(|pool| {
        let index = conflict.is_some_and(|conflict| {
            pool[0].borrow().as_deref().is_some_and(|arena| ptr::eq(conflict, arena))
        }) as usize;
        let arena = pool[index]
            .borrow_mut()
            .get_or_insert_with(|| Rc::new(release::Arena::new(128 * MEBI).unwrap()))
            .clone();
        // The scope owns the arena independently of the TLS pool. It can safely
        // outlive that pool in another TLS destructor without a dangling reference.
        ScratchArena::new(arena)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_scopes_restore_watermarks_and_preserve_outer_data() {
        let outer = scratch_arena(None);
        let original = outer.alloc_uninit_slice::<u64>(128);
        original.fill_with(|| std::mem::MaybeUninit::new(0x1234_5678_9abc_def0));
        let offset = outer.offset();
        for _ in 0..1000 {
            let inner = scratch_arena(None);
            let bytes = inner.alloc_uninit_slice::<u8>(1024);
            bytes.fill(std::mem::MaybeUninit::new(0xff));
        }
        assert_eq!(outer.offset(), offset);
        assert!(unsafe { original.assume_init_ref() }.iter().all(|&n| n == 0x1234_5678_9abc_def0));
    }

    #[test]
    fn live_scopes_prevent_reinitialization() {
        let scratch = scratch_arena(None);
        let value = scratch.alloc_uninit::<u64>().write(42);
        assert!(init(64 * KIBI).is_err());
        assert_eq!(*value, 42);
    }

    #[test]
    fn older_scope_cannot_allocate_while_newer_scope_is_active() {
        let outer = scratch_arena(None);
        let inner = scratch_arena(None);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            outer.alloc_uninit::<u64>();
        }));
        assert!(result.is_err());
        drop(inner);
        outer.alloc_uninit::<u64>().write(42);
    }

    #[test]
    fn concurrent_scratch_pools_are_independent_and_reclaimed() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    init(2 * MEBI).unwrap();
                    let scratch = scratch_arena(None);
                    let bytes = scratch.alloc_uninit_slice::<u8>(64 * KIBI);
                    bytes.fill(std::mem::MaybeUninit::new(index));
                    barrier.wait();
                    assert!(unsafe { bytes.assume_init_ref() }.iter().all(|&b| b == index));
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    }

    #[test]
    fn scope_owns_its_backing_memory_during_tls_teardown() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Survivor(Option<(ScratchArena<'static>, Arc<AtomicBool>)>);
        impl Drop for Survivor {
            fn drop(&mut self) {
                if let Some((scratch, verified)) = self.0.take() {
                    // S_SCRATCH has already been dropped on this thread.
                    let value = scratch.alloc_uninit::<u64>().write(42);
                    assert_eq!(*value, 42);
                    verified.store(true, Ordering::Relaxed);
                }
            }
        }
        thread_local! {
            static SURVIVOR: RefCell<Survivor> = const { RefCell::new(Survivor(None)) };
        }
        let verified = Arc::new(AtomicBool::new(false));
        let thread_verified = verified.clone();
        std::thread::spawn(move || {
            // TLS destructors run in reverse initialization order.
            SURVIVOR.with(|_| {});
            let scratch = scratch_arena(None);
            SURVIVOR.with(|survivor| {
                survivor.borrow_mut().0 = Some((scratch, thread_verified));
            });
        })
        .join()
        .unwrap();
        assert!(verified.load(Ordering::Relaxed));
    }
}
