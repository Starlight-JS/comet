//! # comet-extra
//!
//! Collection of types that can be used with Comet.

#![feature(core_intrinsics, build_hasher_simple_hash_one)]

pub mod alloc;

pub use comet::*;

#[cfg(test)]
pub(crate) fn create_heap_for_tests() -> mutator::MutatorRef<immix::Immix> {
    immix::instantiate_immix(
        128 * 1024 * 1024,
        4 * 1024 * 1024,
        2 * 1024 * 1024,
        128 * 1024 * 1024,
        true,
    )
}
