//! # comet-extra
//!
//! Collection of types that can be used with Comet.

#![feature(core_intrinsics, build_hasher_simple_hash_one)]

pub mod alloc;

pub use comet::*;

#[cfg(test)]
pub(crate) fn create_heap_for_tests() -> mutator::MutatorRef<immix::Immix> {
    use comet::immix::ImmixOptions;

    immix::instantiate_immix(ImmixOptions::default().with_verbose(1))
}
