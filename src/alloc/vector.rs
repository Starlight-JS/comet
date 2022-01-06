use std::{mem::size_of, sync::atomic::AtomicU32};

use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::GcBase,
    mutator::MutatorRef,
};

/// `Vector` is a space-optimized GCed implementation of `alloc::vec::Vec` that is only the size of a single pointer and
/// also extends portions of its API. In many cases, it is a drop-in replacement for the "real" `Vec`.
#[repr(transparent)]
pub struct Vector<T: Trace + 'static, H: GcBase> {
    storage: Gc<VectorStorage<T>, H>,
}

impl<T: Trace + 'static, H: GcBase> Vector<T, H> {
    /// Inserts GC write barrier. Must be invoked after each write to vector.
    pub fn write_barrier(&mut self, mutator: &mut MutatorRef<H>) {
        mutator.write_barrier(self.storage.to_dyn());
    }

    /// Get vector as immutable slice
    pub fn as_slice<'a>(&'a self) -> &'a [T] {
        unsafe { std::slice::from_raw_parts(self.data(), self.len()) }
    }
    /// Get vector as mutable slice
    pub fn as_slice_mut<'a>(&'a mut self) -> &'a mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.data(), self.len()) }
    }
    pub fn new(mutator: &mut MutatorRef<H>) -> Vector<T, H> {
        Vector {
            storage: VectorStorage::create(mutator, 0),
        }
    }

    pub fn with_capacity(mutator: &mut MutatorRef<H>, capacity: usize) -> Vector<T, H> {
        Vector {
            storage: VectorStorage::create(mutator, capacity),
        }
    }

    fn data(&self) -> *mut T {
        self.storage.data_start.as_ptr() as *mut T
    }

    fn grow(&mut self, mutator: &mut MutatorRef<H>, capacity: usize) {
        let old_capacity = self.capacity();
        let new_capacity = capacity;
        if new_capacity == old_capacity {
            return;
        }

        let len = self.len();
        let mut temp = VectorStorage::create(mutator, new_capacity);
        unsafe {
            core::ptr::copy_nonoverlapping(self.data(), temp.data_start.as_mut_ptr(), len);
        }
        mutator.write_barrier(temp.to_dyn());
        self.storage = temp;
    }

    pub fn capacity(&self) -> usize {
        self.storage.capacity.load(atomic::Ordering::Relaxed) as _
    }

    pub fn len(&self) -> usize {
        self.storage.length.load(atomic::Ordering::Relaxed) as _
    }

    pub fn as_mut_ptr(&self) -> *mut T {
        self.data()
    }

    pub fn as_ptr(&self) -> *const T {
        self.data()
    }

    /// `swap_remove` removes the element located at `index` and replaces it with the last value
    /// in the vector, returning the removed element to the caller.
    #[must_use]
    pub fn swap_remove(&mut self, index: usize) -> T {
        let len = self.len();

        assert!(
            (index < len),
            "swap_remove index (is {}) should be < len (is {})",
            index,
            len
        );

        unsafe { core::ptr::swap(self.as_mut_ptr().add(len - 1), self.as_mut_ptr().add(index)) };

        let x = unsafe { core::ptr::read(self.as_ptr().add(self.len() - 1)) };
        self.storage.length.fetch_sub(1, atomic::Ordering::Relaxed);
        x
    }

    pub fn shrink_to(&mut self, mutator: &mut MutatorRef<H>, min_capacity: usize) {
        let len = self.len();
        let capacity = self.capacity();

        if min_capacity < len {
            self.shrink_to_fit(mutator);
            return;
        }

        if capacity == min_capacity {
            return;
        }

        assert!(
            capacity >= min_capacity,
            "Tried to shrink to a larger capacity"
        );

        self.grow(mutator, min_capacity);
    }

    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&T) -> bool,
    {
        let len = self.len();
        let data = self.as_mut_ptr();
        let mut read = data;
        let mut write = read;

        let last = unsafe { data.add(len) };

        while read < last {
            let should_retain = unsafe { f(&mut *read) };
            if should_retain {
                if read != write {
                    unsafe {
                        core::mem::swap(&mut *read, &mut *write);
                    }
                }
                write = unsafe { write.add(1) };
            }

            read = unsafe { read.add(1) };
        }

        self.truncate((write as usize - data as usize) / core::mem::size_of::<T>());
    }
    pub fn retain_mut<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        let original_len = self.len();
        // Avoid double drop if the drop guard is not executed,
        // since we may make some holes during the process.
        unsafe { self.set_len(0) };

        // Vec: [Kept, Kept, Hole, Hole, Hole, Hole, Unchecked, Unchecked]
        //      |<-              processed len   ->| ^- next to check
        //                  |<-  deleted cnt     ->|
        //      |<-              original_len                          ->|
        // Kept: Elements which predicate returns true on.
        // Hole: Moved or dropped element slot.
        // Unchecked: Unchecked valid elements.
        //
        // This drop guard will be invoked when predicate or `drop` of element panicked.
        // It shifts unchecked elements to cover holes and `set_len` to the correct length.
        // In cases when predicate and `drop` never panick, it will be optimized out.
        struct BackshiftOnDrop<'a, T: Trace + 'static, H: GcBase> {
            v: &'a mut Vector<T, H>,
            processed_len: usize,
            deleted_cnt: usize,
            original_len: usize,
        }

        impl<T: Trace + 'static, H: GcBase> Drop for BackshiftOnDrop<'_, T, H> {
            fn drop(&mut self) {
                if self.deleted_cnt > 0 {
                    // SAFETY: Trailing unchecked items must be valid since we never touch them.
                    unsafe {
                        std::ptr::copy(
                            self.v.as_ptr().add(self.processed_len),
                            self.v
                                .as_mut_ptr()
                                .add(self.processed_len - self.deleted_cnt),
                            self.original_len - self.processed_len,
                        );
                    }
                }
                // SAFETY: After filling holes, all items are in contiguous memory.
                unsafe {
                    self.v.set_len(self.original_len - self.deleted_cnt);
                }
            }
        }

        let mut g = BackshiftOnDrop {
            v: self,
            processed_len: 0,
            deleted_cnt: 0,
            original_len,
        };

        fn process_loop<F, T: Trace + 'static, H: GcBase, const DELETED: bool>(
            original_len: usize,
            f: &mut F,
            g: &mut BackshiftOnDrop<'_, T, H>,
        ) where
            F: FnMut(&mut T) -> bool,
        {
            while g.processed_len != original_len {
                // SAFETY: Unchecked element must be valid.
                let cur = unsafe { &mut *g.v.as_mut_ptr().add(g.processed_len) };
                if !f(cur) {
                    // Advance early to avoid double drop if `drop_in_place` panicked.
                    g.processed_len += 1;
                    g.deleted_cnt += 1;
                    // SAFETY: We never touch this element again after dropped.
                    unsafe { std::ptr::drop_in_place(cur) };
                    // We already advanced the counter.
                    if DELETED {
                        continue;
                    } else {
                        break;
                    }
                }
                if DELETED {
                    // SAFETY: `deleted_cnt` > 0, so the hole slot must not overlap with current element.
                    // We use copy for move, and never touch this element again.
                    unsafe {
                        let hole_slot = g.v.as_mut_ptr().add(g.processed_len - g.deleted_cnt);
                        std::ptr::copy_nonoverlapping(cur, hole_slot, 1);
                    }
                }
                g.processed_len += 1;
            }
        }

        // Stage 1: Nothing was deleted.
        process_loop::<F, T, H, false>(original_len, &mut f, &mut g);

        // Stage 2: Some elements were deleted.
        process_loop::<F, T, H, true>(original_len, &mut f, &mut g);

        // All item are processed. This can be optimized to `set_len` by LLVM.
        drop(g);
    }
    pub fn clear(&mut self) {
        self.truncate(0);
    }
    /// `append` moves every element from `other` to the back of `self`. `other.is_empty()` is `true` once this operation
    /// completes and its capacity is unaffected.
    ///
    ///
    /// TODO: Should we assume that `other` is already rooted?
    pub fn append(&mut self, mutator: &mut MutatorRef<H>, other: &mut Vector<T, H>) {
        if other.is_empty() {
            return;
        }

        let other_len = other.len();
        self.reserve(mutator, other_len);

        unsafe {
            core::ptr::copy_nonoverlapping(
                other.as_ptr(),
                self.as_mut_ptr().add(self.len()),
                other_len,
            );
        };

        unsafe {
            other.set_len(0);
            self.set_len(self.len() + other_len);
        };
    }
    pub fn resize(&mut self, mutator: &mut MutatorRef<H>, new_len: usize, value: T)
    where
        T: Clone,
    {
        let len = self.len();

        match new_len.cmp(&len) {
            core::cmp::Ordering::Equal => {}
            core::cmp::Ordering::Greater => {
                let stack = mutator.shadow_stack();
                // root value because it might contain GC pointer
                // and when we invoke `push()` GC might be triggered and this GC pointer can be moved.
                letroot!(value = stack, value);
                let num_elems = new_len - len;
                self.reserve(mutator, num_elems);
                for _i in 0..num_elems {
                    self.push(mutator, value.clone());
                }
            }
            core::cmp::Ordering::Less => {
                self.truncate(new_len);
            }
        }
    }
    /// `resize_with` will invoke the supplied callable `f` as many times as is required until
    /// `len() == new_len` is true. If the `new_len` exceeds the current [`len()`](MiniVec::len)
    /// then the vector will be resized via a call to `truncate(new_len)`. If the `new_len` and
    /// `len()` are equal then no operation is performed.
    pub fn resize_with<F>(&mut self, mutator: &mut MutatorRef<H>, new_len: usize, mut f: F)
    where
        F: FnMut(&mut MutatorRef<H>) -> T,
    {
        use core::cmp::Ordering::{Equal, Greater, Less};

        let len = self.len();
        match new_len.cmp(&len) {
            Equal => {}
            Greater => {
                let num_elems = new_len - len;
                self.reserve(mutator, num_elems);
                for _i in 0..num_elems {
                    let val = f(mutator); // do not root `val` because it is rooted in `push()` already
                    self.push(mutator, val);
                }
            }
            Less => {
                self.truncate(new_len);
            }
        }
    }
    #[allow(clippy::ptr_as_ptr)]
    pub fn split_off(&mut self, mutator: &mut MutatorRef<H>, at: usize) -> Self {
        let len = self.len();

        assert!(
            (at <= len),
            "`at` split index (is {}) should be <= len (is {})",
            at,
            len
        );

        if len == 0 {
            let other = if self.capacity() > 0 {
                Self::with_capacity(mutator, self.capacity())
            } else {
                Self::new(mutator)
            };

            return other;
        }
        let stack = mutator.shadow_stack();

        if at == 0 {
            let orig_cap = self.capacity();

            letroot!(
                other = stack,
                Some(Self {
                    storage: self.storage,
                })
            );

            self.storage = VectorStorage::create(mutator, 0);
            self.reserve_exact(mutator, orig_cap);

            return other.take().unwrap();
        }

        letroot!(
            other = stack,
            Some(Self::with_capacity(mutator, self.capacity()))
        );

        unsafe {
            self.set_len(at);
            other.as_mut().unwrap().set_len(len - at);
        }

        let src = unsafe { self.as_ptr().add(at) };
        let dst = other.as_mut().unwrap().as_mut_ptr();
        let count = len - at;

        unsafe {
            core::ptr::copy_nonoverlapping(src, dst, count);
        }

        other.take().unwrap()
    }
    /// `reserve_exact` ensures that the capacity of the vector is exactly equal to
    /// `len() + additional` unless the capacity is already sufficient in which case no operation is
    /// performed.
    ///
    pub fn reserve_exact(&mut self, mutator: &mut MutatorRef<H>, additional: usize) {
        let capacity = self.capacity();
        let len = self.len();

        let total_required = len + additional;
        if capacity >= total_required {
            return;
        }

        self.grow(mutator, total_required);
    }

    /// `truncate` adjusts the length of the vector to be `len`. If `len` is greater than or equal
    /// to the current length no operation is performed. Otherwise, the vector's length is
    /// readjusted to `len` and any remaining elements to the right of `len` are dropped.
    pub fn truncate(&mut self, len: usize) {
        let self_len = self.len();
        if len >= self_len {
            return;
        }

        self.storage.length.store(0, atomic::Ordering::Relaxed);
        if !core::mem::needs_drop::<T>() {
            return;
        }

        let s = unsafe { std::slice::from_raw_parts_mut(self.data().add(len), self_len - len) };
        unsafe {
            core::ptr::drop_in_place(s);
        }
    }
    /// `push` appends an element `value` to the end of the vector. `push` automatically reallocates
    /// if the vector does not have sufficient capacity.
    ///
    /// **NOTE**: You must insert write barrier if vector holds GC data.
    pub fn push(&mut self, mutator: &mut MutatorRef<H>, value: T) {
        let len = self.len();
        let cap = self.capacity();
        let stack = mutator.shadow_stack();
        letroot!(value = stack, Some(value));
        if len == cap {
            self.grow(mutator, next_capacity::<T>(cap));
        }

        let data = self.data();
        unsafe {
            data.write(value.take().unwrap());
        }
        self.storage.length.fetch_add(1, atomic::Ordering::AcqRel);
    }
    pub fn extend_from_slice(&mut self, mutator: &mut MutatorRef<H>, slice: &mut [T])
    where
        T: Clone,
    {
        let stack = mutator.shadow_stack();
        letroot!(slice = stack, slice);
        self.reserve(mutator, slice.len());
        for x in (*slice).iter() {
            self.push(mutator, (*x).clone());
        }
    }
    pub fn pop(&mut self) -> Option<T> {
        let len = self.len();
        if len == 0 {
            return None;
        }

        unsafe {
            let v = self.as_ptr().add(len - 1).read();
            self.storage.length.fetch_sub(1, atomic::Ordering::Relaxed);
            Some(v)
        }
    }

    pub unsafe fn set_len(&mut self, len: usize) {
        self.storage
            .length
            .store(len as _, atomic::Ordering::Release);
    }

    pub fn remove(&mut self, index: usize) -> T {
        let len = self.len();

        assert!(
            (index < len),
            "removal index (is {}) should be < len (is {})",
            index,
            len
        );

        unsafe {
            let p = self.as_mut_ptr().add(index);

            let x = core::ptr::read(p);

            let src = p.add(1);
            let dst = p;
            let count = len - index - 1;
            core::ptr::copy(src, dst, count);

            self.set_len(len - 1);

            x
        }
    }

    /// `remove_item` removes the first element identical to the supplied `item` using a
    /// left-to-right traversal of the elements.
    ///
    pub fn remove_item<V>(&mut self, item: &V) -> Option<T>
    where
        T: PartialEq<V>,
    {
        let len = self.len();
        for i in 0..len {
            if self.at(i) == item {
                return Some(self.remove(i));
            }
        }
        None
    }
    pub fn try_reserve(&mut self, mutator: &mut MutatorRef<H>, additional: usize) {
        let capacity = self.capacity();
        let total_required = self.len().saturating_add(additional);

        if total_required <= capacity {
            return;
        }

        let mut new_capacity = next_capacity::<T>(capacity);
        while new_capacity < total_required {
            new_capacity = next_capacity::<T>(new_capacity);
        }

        self.grow(mutator, new_capacity);
    }

    pub fn at(&self, index: usize) -> &T {
        unsafe { &*self.data().add(index) }
    }

    pub fn at_mut(&mut self, index: usize) -> &mut T {
        unsafe { &mut *self.data().add(index) }
    }

    pub fn reserve(&mut self, mutator: &mut MutatorRef<H>, additional: usize) {
        self.try_reserve(mutator, additional);
    }

    pub fn shrink_to_fit(&mut self, mutator: &mut MutatorRef<H>) {
        let len = self.len();
        if len == self.capacity() {
            return;
        }

        self.grow(mutator, len);
    }

    pub fn insert(&mut self, mutator: &mut MutatorRef<H>, index: usize, element: T) {
        let len = self.len();

        assert!(
            (index <= len),
            "insertion index (is {}) should be <= len (is {})",
            index,
            len
        );

        if len == self.capacity() {
            self.reserve(mutator, 1);
        }

        let p = unsafe { self.as_mut_ptr().add(index) };
        unsafe {
            core::ptr::copy(p, p.add(1), len - index);
            core::ptr::write(p, element);
            self.set_len(len + 1);
        }
    }
    /// `dedup_by` "de-duplicates" all adjacent elements for which the supplied binary predicate
    /// returns true.

    #[allow(clippy::cast_sign_loss)]
    pub fn dedup_by<F>(&mut self, mut pred: F)
    where
        F: FnMut(&mut T, &mut T) -> bool,
    {
        struct DropGuard<'a, T: Trace + 'static, H: GcBase> {
            read: *const T,
            write: *mut T,
            last: *const T,
            len: usize,
            vec: &'a mut Vector<T, H>,
        }

        impl<'a, T: Trace + 'static, H: GcBase> Drop for DropGuard<'a, T, H> {
            fn drop(&mut self) {
                if self.read != self.write {
                    let src = self.read;
                    let dst = self.write;
                    let count = unsafe { self.last.offset_from(self.read) as usize };
                    unsafe { core::ptr::copy(src, dst, count) };
                }

                unsafe { self.vec.set_len(self.len) };
            }
        }

        let mut len = self.len();
        if len < 2 {
            return;
        }

        let data = self.as_mut_ptr();

        let mut read = unsafe { data.add(1) };
        let mut write = read;

        let last = unsafe { data.add(len) };

        while read < last {
            let mut guard = DropGuard {
                read,
                write,
                last,
                len,
                vec: self,
            };

            let matches = unsafe { pred(&mut *read, &mut *write.sub(1)) };
            if matches {
                let v = unsafe { core::ptr::read(read) };
                len -= 1;
                guard.len -= 1;
                guard.read = unsafe { guard.read.add(1) };

                core::mem::drop(v);
            } else {
                if read != write {
                    let src = read;
                    let dst = write;
                    let count = 1;
                    unsafe { core::ptr::copy(src, dst, count) };
                }

                write = unsafe { write.add(1) };
            }

            read = unsafe { read.add(1) };

            core::mem::forget(guard);
        }

        unsafe { self.set_len(len) };
    }
    pub fn dedup_by_key<F, K>(&mut self, mut key: F)
    where
        F: FnMut(&mut T) -> K,
        K: PartialEq<K>,
    {
        self.dedup_by(|a, b| key(a) == key(b));
    }
    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        self.dedup_by(|x, y| x == y);
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

unsafe impl<T: Trace, H: GcBase> Trace for Vector<T, H> {
    fn trace(&mut self, _vis: &mut dyn crate::api::Visitor) {
        self.storage.trace(_vis);
    }
}

#[repr(C)]
struct VectorStorage<T: Trace + 'static> {
    length: AtomicU32,
    capacity: AtomicU32,
    data_start: [T; 0],
}

impl<T: Trace + 'static> VectorStorage<T> {
    fn create<H: GcBase>(mutator: &mut MutatorRef<H>, capacity: usize) -> Gc<Self, H> {
        let this = Self {
            length: AtomicU32::new(0),
            capacity: AtomicU32::new(capacity as u32),
            data_start: [],
        };
        mutator.allocate(this, crate::gc_base::AllocationSpace::New)
    }
}

unsafe impl<T: Trace + 'static> Trace for VectorStorage<T> {
    fn trace(&mut self, vis: &mut dyn crate::api::Visitor) {
        unsafe {
            let mut cursor = self.data_start.as_mut_ptr();
            let end = cursor.add(self.length.load(atomic::Ordering::Acquire) as _);
            while cursor < end {
                (*cursor).trace(vis);
                cursor = cursor.add(1);
            }
        }
    }
}
unsafe impl<T: Trace + 'static> Finalize for VectorStorage<T> {}
impl<T: Trace + 'static> Collectable for VectorStorage<T> {
    fn allocation_size(&self) -> usize {
        self.capacity.load(atomic::Ordering::Relaxed) as usize * size_of::<T>() + size_of::<Self>()
    }
}

const fn next_capacity<T>(capacity: usize) -> usize {
    let elem_size = core::mem::size_of::<T>();

    if capacity == 0 {
        return match elem_size {
            1 => 8,
            2..=1024 => 4,
            _ => 1,
        };
    }

    2 * capacity
}

#[macro_export]
macro_rules! gc_vector {
    ($mutator: expr) => {
        $crate::alloc::vector::Vector::new(&mut $mutator)
    };
    ($mutator: expr; [$elem: expr;$count: expr]) => {{
        let stack = $mutator.shadow_stack();
        $crate::letroot!(vec = stack, Some($crate::alloc::vector::Vector::with_capacity(&mut $mutator,$count)));

        for _ in 0..$count {
            vec.as_mut().unwrap().push(&mut $mutator,$elem);
        }

        vec.take().unwrap()
    }};
    ($mutator: expr; $($x: expr),+$(,)?) => {{
        let stack = $mutator.shadow_stack();
        $crate::letroot!(vec = stack, Some($crate::alloc::vector::Vector::new(&mut $mutator)));

        $(
            vec.as_mut().unwrap().push(&mut $mutator,$x);
            $mutator.write_barrier(*vec);
        )*
        vec.take().unwrap()
    }}
}

impl<T: std::fmt::Debug + Trace, H: GcBase> std::fmt::Debug for Vector<T, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Vector[")?;
        for i in 0..self.len() {
            write!(f, "{:?}", self.at(i))?;
            if i != self.len() - 1 {
                write!(f, ",")?;
            }
        }

        write!(f, "]")
    }
}

impl<T: PartialEq + Trace, H: GcBase> PartialEq for Vector<T, H> {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }

        for i in 0..self.len() {
            if self.at(i) != other.at(i) {
                return false;
            }
        }
        true
    }
}

impl<T: Eq + Trace, H: GcBase> Eq for Vector<T, H> {}

impl<T: Trace, I, H: GcBase> core::ops::Index<I> for Vector<T, H>
where
    I: core::slice::SliceIndex<[T]>,
{
    type Output = <I as core::slice::SliceIndex<[T]>>::Output;

    fn index(&self, index: I) -> &<Vector<T, H> as core::ops::Index<I>>::Output {
        let v: &[T] = &**self;
        core::ops::Index::index(v, index)
    }
}

impl<T: Trace, I, H: GcBase> core::ops::IndexMut<I> for Vector<T, H>
where
    I: core::slice::SliceIndex<[T]>,
{
    fn index_mut(&mut self, index: I) -> &mut <Vector<T, H> as core::ops::Index<I>>::Output {
        let v: &mut [T] = &mut **self;
        core::ops::IndexMut::index_mut(v, index)
    }
}

impl<T: Trace, H: GcBase> core::ops::Deref for Vector<T, H> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        let data = self.data();
        let len = self.len();
        unsafe { core::slice::from_raw_parts(data, len) }
    }
}

impl<T: Trace, H: GcBase> core::ops::DerefMut for Vector<T, H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let data = self.data();
        let len = self.len();
        unsafe { core::slice::from_raw_parts_mut(data, len) }
    }
}

impl<T: Trace, G: GcBase> core::hash::Hash for Vector<T, G>
where
    T: core::hash::Hash,
{
    fn hash<H>(&self, state: &mut H)
    where
        H: core::hash::Hasher,
    {
        let this: &[T] = &**self;
        core::hash::Hash::hash(this, state);
    }
}
