use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash},
};

use ahash::RandomState;
use comet::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::GcBase,
    letroot,
    mutator::MutatorRef,
};

const THRESHOLD: f64 = 0.75;

use super::vector::Vector;

struct Entry<Key: Trace + 'static, Value: Trace + 'static, H: GcBase> {
    key: Key,
    value: Value,
    hash: u64,
    next: Option<Gc<Self, H>>,
}

unsafe impl<Key: Trace, Value: Trace, H: GcBase> Trace for Entry<Key, Value, H> {
    fn trace(&mut self, vis: &mut dyn comet::api::Visitor) {
        self.key.trace(vis);
        self.value.trace(vis);
        self.next.trace(vis);
    }
}

unsafe impl<Key: Trace, Value: Trace, H: GcBase> Finalize for Entry<Key, Value, H> {}

impl<Key: Trace + 'static, Value: Trace + 'static, H: GcBase> Collectable for Entry<Key, Value, H> {}

pub struct HashMap<Key: Trace + 'static, Value: Trace + 'static, H: GcBase, S = RandomState> {
    hash_builder: S,
    len: usize,
    table: Vector<Option<Gc<Entry<Key, Value, H>, H>>, H>,
}

pub type DefaultHashBuilder = ahash::RandomState;

impl<Key: Trace + 'static, Value: Trace + 'static, H: GcBase>
    HashMap<Key, Value, H, DefaultHashBuilder>
{
    pub fn new(mutator: &mut MutatorRef<H>) -> Self {
        Self::with_hasher(mutator, DefaultHashBuilder::default())
    }

    pub fn with_capacity(mutator: &mut MutatorRef<H>, capacity: usize) -> Self {
        Self::with_capacity_and_hasher(mutator, capacity, DefaultHashBuilder::default())
    }
}
impl<Key: Trace + 'static, Value: Trace + 'static, H: GcBase, S> HashMap<Key, Value, H, S> {
    pub fn with_capacity_and_hasher(
        mutator: &mut MutatorRef<H>,
        capacity: usize,
        hash_builder: S,
    ) -> Self {
        Self {
            hash_builder,
            len: 0,
            table: Vector::with_capacity(mutator, capacity),
        }
    }
    pub fn with_hasher(mutator: &mut MutatorRef<H>, hash_builder: S) -> Self {
        Self {
            hash_builder,
            len: 0,
            table: Vector::with_capacity(mutator, 0),
        }
    }

    fn resize(&mut self, mutator: &mut MutatorRef<H>) {
        let size = self.table.len();
        self.table.resize(mutator, size * 2, None);
    }

    #[inline]
    pub fn insert(&mut self, mutator: &mut MutatorRef<H>, key: Key, value: Value) -> bool
    where
        S: BuildHasher,
        Key: Hash + Eq,
    {
        let hash = make_hash::<&Key, Key, S>(&self.hash_builder, &key);
        let position = (hash % self.table.len() as u64) as usize;

        let mut node = self.table.at_mut(position);
        while let Some(n) = node {
            if n.hash == hash && key == n.key {
                n.value = value;
                return false;
            }
            node = &mut n.next;
        }

        self.insert_slow(mutator, position, hash, key, value);

        true
    }

    fn insert_slow(
        &mut self,
        mutator: &mut MutatorRef<H>,
        mut position: usize,
        hash: u64,
        key: Key,
        value: Value,
    ) where
        Key: Hash + Eq,
    {
        let stack = mutator.shadow_stack();
        // GC might happend and Key or Value might be GC things, protect them.
        letroot!(key = stack, Some(key));
        letroot!(value = stack, Some(value));
        if self.len >= (self.table.len() as f64 * THRESHOLD) as usize {
            self.resize(mutator);
            position = (hash % self.table.len() as u64) as usize;
        }

        let node = mutator.allocate(
            Entry::<Key, Value, H> {
                hash,
                value: value.take().unwrap(),
                key: key.take().unwrap(),
                next: *self.table.at(position),
            },
            comet::gc_base::AllocationSpace::New,
        );
        *self.table.at_mut(position) = Some(node);
        self.len += 1;
    }

    pub fn get(&self, key: &Key) -> Option<&Value>
    where
        Key: Hash + Eq,
        S: BuildHasher,
    {
        let hash = make_hash::<&Key, Key, S>(&self.hash_builder, key);
        let position = (hash % self.table.len() as u64) as usize;
        let mut node = self.table.at(position);
        while let Some(n) = node {
            if n.hash == hash && &n.key == key {
                return Some(&n.value);
            }
            node = &n.next;
        }
        None
    }

    pub fn get_mut(&mut self, key: &Key) -> Option<&mut Value>
    where
        Key: Hash + Eq,
        S: BuildHasher,
    {
        let hash = make_hash::<&Key, Key, S>(&self.hash_builder, key);
        let position = (hash % self.table.len() as u64) as usize;
        let mut node = self.table.at_mut(position);
        while let Some(n) = node {
            if n.hash == hash && &n.key == key {
                return Some(&mut n.value);
            }
            node = &mut n.next;
        }
        None
    }

    pub fn remove(&mut self, key: &Key) -> bool
    where
        Key: Hash + Eq,
        S: BuildHasher,
    {
        let hash = make_hash::<&Key, Key, S>(&self.hash_builder, key);
        let position = (hash % self.table.len() as u64) as usize;
        let mut node = self.table.at_mut(position);
        let mut prevnode: Option<Gc<Entry<Key, Value, H>, H>> = None;
        while let Some(n) = node {
            if n.hash == hash && &n.key == key {
                if let Some(mut prevnode) = prevnode {
                    prevnode.next = n.next;
                } else {
                    *self.table.at_mut(position) = n.next;
                }
                self.len -= 1;
                return true;
            }
            prevnode = Some(*n);
            node = &mut n.next;
        }
        false
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn capacity(&self) -> usize {
        self.table.len()
    }

    pub fn for_each(&self, mut callback: impl FnMut(&Key, &Value)) {
        for i in 0..self.table.len() {
            let mut node = &self.table[i];
            if node.is_none() {
                continue;
            }
            while let Some(n) = node {
                callback(&n.key, &n.value);
                node = &n.next;
            }
        }
    }
}

#[inline]
pub(crate) fn make_hash<K, Q, S>(hash_builder: &S, val: &Q) -> u64
where
    K: Borrow<Q>,
    Q: Hash + ?Sized,
    S: BuildHasher,
{
    hash_builder.hash_one(val)
}

unsafe impl<Key: Trace + 'static, Value: Trace + 'static, H: GcBase, S> Trace
    for HashMap<Key, Value, H, S>
{
    fn trace(&mut self, vis: &mut dyn comet::api::Visitor) {
        self.table.trace(vis);
    }
}
