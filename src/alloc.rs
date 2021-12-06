//! Module that contains types allocated on GC heap: strings, vectors, hashmap etc.

use crate::api::{Collectable, Finalize, Trace};

pub mod fixed_array;
pub mod indexmap;
pub mod string;
pub mod vector;
pub type DefaultHasher = ahash::RandomState;

unsafe impl Trace for ahash::RandomState {}
unsafe impl Finalize for ahash::RandomState {}
impl Collectable for ahash::RandomState {}
