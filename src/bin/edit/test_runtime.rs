//! Keep buffer-backed application fixtures serialized while initializing each
//! test thread's independent scratch pool with a bounded capacity.

use std::sync::{Mutex, MutexGuard, Once};

pub fn lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    thread_local! { static INIT: Once = const { Once::new() }; }
    let guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    INIT.with(|init| init.call_once(|| ogedit::arena::init(32 * 1024 * 1024).unwrap()));
    guard
}
