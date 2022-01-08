use super::array::Array;
use ahash::RandomState;
use comet::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::GcBase,
    letroot,
    mutator::MutatorRef,
};
use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash},
};
const THRESHOLD: f64 = 0.75;

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
    table: Gc<Array<Option<Gc<Entry<Key, Value, H>, H>>>, H>,
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
        mut capacity: usize,
        hash_builder: S,
    ) -> Self {
        if capacity < 1 {
            capacity = 4;
        }
        Self {
            hash_builder,
            len: 0,
            table: Array::new_with_default(mutator, capacity),
        }
    }
    pub fn with_hasher(mut mutator: &mut MutatorRef<H>, hash_builder: S) -> Self {
        Self {
            hash_builder,
            len: 0,
            table: Array::from_slice(&mut mutator, [None; 4]),
        }
    }

    fn resize(&mut self, mutator: &mut MutatorRef<H>) {
        let stack = mutator.shadow_stack();
        letroot!(prev_table = stack, self.table);
        self.table = Array::new_with_default(mutator, self.capacity() * 2);
        let new_len = self.table.len();
        let mut node;
        let mut next;
        for i in 0..prev_table.len() {
            node = prev_table[i];
            while let Some(mut n) = node {
                next = n.next;

                let pos = (n.hash % new_len as u64) as usize;
                n.next = self.table[pos];
                self.table[pos] = Some(n);
                node = next;
            }
        }
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
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn contains_key(&self, key: &Key) -> bool
    where
        Key: Hash + Eq,
        S: BuildHasher,
    {
        let hash = make_hash::<&Key, Key, S>(&self.hash_builder, key);
        let position = (hash % self.table.len() as u64) as usize;
        let mut node = self.table.at(position);
        while let Some(n) = node {
            if n.hash == hash && &n.key == key {
                return true;
            }
            node = &n.next;
        }

        false
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

    pub fn iter(&self) -> HashMapConstIterator<'_, Key, Value, H, S> {
        HashMapConstIterator {
            current: None,
            map: self,
            index: 0,
        }
    }

    pub fn iter_mut(&mut self) -> HashMapMutIterator<'_, Key, Value, H, S> {
        HashMapMutIterator {
            current: None,
            map: self,
            index: 0,
        }
    }
}

pub struct HashMapConstIterator<'a, K: Trace + 'static, V: Trace + 'static, H: GcBase, S> {
    map: &'a HashMap<K, V, H, S>,
    current: Option<&'a Option<Gc<Entry<K, V, H>, H>>>,
    index: usize,
}

impl<'a, K: Trace + 'static, V: Trace + 'static, H: GcBase, S> Iterator
    for HashMapConstIterator<'a, K, V, H, S>
{
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(current) = self.current {
            if let Some(cur) = current {
                self.current = Some(&cur.next);
                return Some((&cur.key, &cur.value));
            }
        }
        if self.index >= self.map.capacity() {
            return None;
        }
        self.current = Some(&self.map.table[self.index]);
        self.index += 1;
        self.next()
    }
}

pub struct HashMapMutIterator<'a, K: Trace + 'static, V: Trace + 'static, H: GcBase, S> {
    map: &'a mut HashMap<K, V, H, S>,
    current: Option<*mut Option<Gc<Entry<K, V, H>, H>>>,
    index: usize,
}

impl<'a, K: Trace + 'static, V: Trace + 'static, H: GcBase, S> Iterator
    for HashMapMutIterator<'a, K, V, H, S>
{
    type Item = (&'a K, &'a mut V);
    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            if let Some(current) = self.current {
                if let Some(cur) = &mut *current {
                    let key = &mut cur.key as *mut K;
                    let val = &mut cur.value as *mut V;
                    self.current = Some(&mut cur.next);
                    return Some((&*key, &mut *val));
                }
            }
            if self.index >= self.map.capacity() {
                return None;
            }
            self.current = Some(&mut self.map.table[self.index]);
            self.index += 1;
            self.next()
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
unsafe impl<Key: Trace + 'static, Value: Trace + 'static, H: GcBase, S> Finalize
    for HashMap<Key, Value, H, S>
{
}

impl<Key: Trace + 'static, Value: Trace + 'static, H: GcBase, S: 'static> Collectable
    for HashMap<Key, Value, H, S>
{
}
#[cfg(test)]
mod test_map {
    use comet::letroot;

    use crate::{alloc::hash::HashMap, create_heap_for_tests};

    #[test]
    fn test_insert() {
        let mut heap = create_heap_for_tests();
        letroot!(m = heap.shadow_stack(), HashMap::new(&mut heap));

        assert_eq!(m.len(), 0);
        assert!(m.insert(&mut heap, 1, 2));
        assert_eq!(m.len(), 1);
        assert!(m.insert(&mut heap, 2, 4));
        assert_eq!(m.len(), 2);
        assert_eq!(*m.get(&1).unwrap(), 2);
        assert_eq!(*m.get(&2).unwrap(), 4);
    }
    #[test]
    fn test_lots_of_insertions() {
        let mut heap = create_heap_for_tests();
        letroot!(
            m = heap.shadow_stack(),
            HashMap::<i32, i32, _, _>::new(&mut heap)
        );
        // Try this a few times to make sure we never screw up the hashmap's
        // internal state.
        for _ in 0..10 {
            assert!(m.is_empty());

            for i in 1..1001 {
                assert!(m.insert(&mut heap, i, i));

                for j in 1..=i {
                    let r = m.get(&j);
                    assert_eq!(r, Some(&j));
                }

                for j in i + 1..1001 {
                    let r = m.get(&j);
                    assert_eq!(r, None);
                }
            }

            for i in 1001..2001 {
                assert!(!m.contains_key(&i));
            }

            // remove forwards
            for i in 1..1001 {
                assert!(m.remove(&i));

                for j in 1..=i {
                    assert!(!m.contains_key(&j));
                }

                for j in i + 1..1001 {
                    assert!(m.contains_key(&j));
                }
            }

            for i in 1..1001 {
                assert!(!m.contains_key(&i));
            }

            for i in 1..1001 {
                assert!(m.insert(&mut heap, i, i));
            }

            // remove backwards
            for i in (1..1001).rev() {
                assert!(m.remove(&i));

                for j in i..1001 {
                    assert!(!m.contains_key(&j));
                }

                for j in 1..i {
                    assert!(m.contains_key(&j));
                }
            }
        }
    }
}

pub struct HashSet<K: Trace + 'static, H: GcBase, S = RandomState> {
    map: HashMap<K, (), H, S>,
}

impl<K: Trace + 'static, H: GcBase, S> HashSet<K, H, S> {
    pub fn with_capacity_and_hasher(
        mutator: &mut MutatorRef<H>,
        capacity: usize,
        hash_builder: S,
    ) -> Self {
        Self {
            map: HashMap::with_capacity_and_hasher(mutator, capacity, hash_builder),
        }
    }

    pub fn with_hasher(mutator: &mut MutatorRef<H>, hash_builder: S) -> Self {
        Self {
            map: HashMap::with_hasher(mutator, hash_builder),
        }
    }

    pub fn insert(&mut self, mutator: &mut MutatorRef<H>, key: K) -> bool
    where
        S: BuildHasher,
        K: Hash + Eq,
    {
        self.map.insert(mutator, key, ())
    }

    pub fn contains(&self, key: &K) -> bool
    where
        S: BuildHasher,
        K: Hash + Eq,
    {
        self.map.contains_key(key)
    }

    pub fn remove(&mut self, key: &K) -> bool
    where
        S: BuildHasher,
        K: Hash + Eq,
    {
        self.map.remove(key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn capacity(&self) -> usize {
        self.map.capacity()
    }

    pub fn iter(&self) -> HashSetIterator<'_, K, H, S> {
        HashSetIterator {
            iter: self.map.iter(),
        }
    }
}

pub struct HashSetIterator<'a, K: Trace + 'static, H: GcBase, S> {
    iter: HashMapConstIterator<'a, K, (), H, S>,
}

impl<'a, K: Trace + 'static, H: GcBase, S> Iterator for HashSetIterator<'a, K, H, S> {
    type Item = &'a K;
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|x| x.0)
    }
}

impl<K: Trace + 'static, H: GcBase> HashSet<K, H, DefaultHashBuilder> {
    pub fn new(mutator: &mut MutatorRef<H>) -> Self {
        Self {
            map: HashMap::new(mutator),
        }
    }

    pub fn with_capacity(mutator: &mut MutatorRef<H>, capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(mutator, capacity),
        }
    }
}
unsafe impl<Key: Trace + 'static, H: GcBase, S> Trace for HashSet<Key, H, S> {
    fn trace(&mut self, vis: &mut dyn comet::api::Visitor) {
        self.map.trace(vis);
    }
}
unsafe impl<Key: Trace + 'static, H: GcBase, S> Finalize for HashSet<Key, H, S> {}

impl<Key: Trace + 'static, H: GcBase, S: 'static> Collectable for HashSet<Key, H, S> {}
