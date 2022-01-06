//! Global GC instance. This module allows you to have global GC instance that is local per process.

use std::sync::atomic::AtomicBool;

use crate::{
    api::{Collectable, Gc, Weak},
    gc_base::AllocationSpace,
    immix::{self, Immix},
    mutator::{JoinData, MutatorRef},
};

#[thread_local]
static mut MUTATOR: Option<MutatorRef<Immix>> = None;
static INIT: AtomicBool = AtomicBool::new(false);

/// Initialize global GC state. This will initialize Immix GC as global one.
///
/// # Panics
/// Panics is GC state is already initialized.
pub fn global_initialize(
    heap_size: usize,
    initial_size: usize,
    min_free: usize,
    max_free: usize,
    verbose: bool,
) -> MutatorRef<Immix> {
    if INIT.load(atomic::Ordering::Acquire) {
        panic!("global GC is already initialized");
    }

    unsafe {
        MUTATOR = Some(immix::instantiate_immix(
            heap_size,
            initial_size,
            min_free,
            max_free,
            verbose,
        ));
        MUTATOR.as_ref().unwrap().clone()
    }
}

/// Allocates `value` on GC heap.
///
/// # Safety
///
/// Unsafe to call because it does not check for TLS state of mutator to be initialized.  
///
pub unsafe fn allocate<T: Collectable + Sized + 'static>(
    value: T,
    space: AllocationSpace,
) -> Gc<T, Immix> {
    let mut mutator = mutator();

    mutator.allocate(value, space)
}

/// Creates weak reference on GC heap.
///
/// # Safety
///
/// Unsafe to call because it does not check for TLS state of mutator to be initialized.  
///
pub unsafe fn allocate_weak<T: Collectable>(object: Gc<T, Immix>) -> Weak<T, Immix> {
    let mut mutator = mutator();
    mutator.allocate_weak(object)
}

/// Get mutator reference.
///
///
/// # Safety
///
/// Unsafe because does not check for initialization of global GC.
pub unsafe fn mutator() -> MutatorRef<Immix> {
    MUTATOR.as_ref().unwrap_unchecked().clone()
}

/// Spawns mutator thread that is attached to GC heap. You should invoke this when using global GC so TLS mutator state is initialized properly.
pub fn spawn_mutator(
    mutator: &mut MutatorRef<Immix>,
    callback: impl FnOnce(&mut MutatorRef<Immix>) + Send + 'static,
) -> JoinData {
    mutator.spawn_mutator(move |mut mutator| unsafe {
        MUTATOR = Some(mutator.clone());
        callback(&mut mutator);
        drop(MUTATOR.take());
    })
}

/// Inserts GC safepoint into your code. Returns true when thread stops at safepoint.
///
///
/// # Safety
///
/// Unsafe function because it does not check for TLS mutator state.
pub unsafe fn safepoint() -> bool {
    MUTATOR.as_ref().unwrap_unchecked().safepoint()
}
