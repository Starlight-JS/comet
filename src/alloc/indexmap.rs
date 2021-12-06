/*use std::{
    hash::{BuildHasher, Hash, Hasher},
    marker::PhantomData,
};

use super::vector::Vector;

use crate::api::{Collectable, Finalize, Gc, Trace};

pub struct GcHashMap<
    K: Collectable + Hash,
    V: Collectable,
    B: BuildHasher + Collectable = super::DefaultHasher,
    H: crate::base::GcBase + 'static = crate::Heap,
> {
    table: Gc<Vector<Option<Gc<Node<K, V>>>, H>>,
    count: usize,
    hasher: B,
    marker: PhantomData<H>,
}

#[repr(C, align(8))]
struct Node<K: Collectable + Hash, V: Collectable> {
    key: K,
    value: V,
    next: Option<Gc<Self>>,
}
unsafe impl<K: Collectable + Hash, V: Collectable> Trace for Node<K, V> {
    fn trace(&mut self, vis: &mut dyn crate::api::Visitor) {
        self.key.trace(vis);
        self.value.trace(vis);
        match self.next {
            Some(ref mut next) => next.trace(vis),
            _ => (),
        }
    }
}

unsafe impl<K: Collectable + Hash, V: Collectable> Finalize for Node<K, V> {}

impl<K: Collectable + Hash, V: Collectable> Collectable for Node<K, V> {}

unsafe impl<
        K: Collectable + Hash,
        V: Collectable,
        B: BuildHasher + Collectable,
        H: crate::base::GcBase,
    > Trace for GcHashMap<K, V, B, H>
{
    fn trace(&mut self, vis: &mut dyn crate::api::Visitor) {
        self.table.trace(vis);
    }
}

unsafe impl<
        K: Collectable + Hash,
        V: Collectable,
        B: BuildHasher + Collectable,
        H: crate::base::GcBase,
    > Finalize for GcHashMap<K, V, B, H>
{
}
impl<
        K: Collectable + Hash,
        V: Collectable,
        B: BuildHasher + Collectable + 'static,
        H: crate::base::GcBase + 'static,
    > Collectable for GcHashMap<K, V, B, H>
{
}
*/
