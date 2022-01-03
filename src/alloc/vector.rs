use std::{mem::size_of, sync::atomic::AtomicU32};

use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::GcBase,
    mutator::MutatorRef,
};

#[repr(transparent)]
pub struct Vector<T: Trace + 'static> {
    storage: Gc<VectorStorage<T>>,
}

impl<T: Trace + 'static> Vector<T> {
    pub fn new(mutator: &mut MutatorRef<impl GcBase>) -> Self {
        Self {
            storage: VectorStorage::create(mutator, 0),
        }
    }

    pub fn with_capacity(mutator: &mut MutatorRef<impl GcBase>, capacity: usize) -> Self {
        Self {
            storage: VectorStorage::create(mutator, capacity),
        }
    }

    fn data(&self) -> *mut T {
        self.storage.data_start.as_ptr() as *mut T
    }

    fn grow(&mut self, mutator: &mut MutatorRef<impl GcBase>, capacity: usize) {
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

    pub fn push(&mut self, mutator: &mut MutatorRef<impl GcBase>, value: T) {
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
    /// # Example
    ///
    /// ```
    /// let mut vec = minivec::mini_vec![0, 1, 1, 1, 2, 3, 4];
    /// vec.remove_item(&1);
    ///
    /// assert_eq!(vec, [0, 1, 1, 2, 3, 4]);
    /// ```
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
    pub fn try_reserve(&mut self, mutator: &mut MutatorRef<impl GcBase>, additional: usize) {
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
}

unsafe impl<T: Trace> Trace for Vector<T> {
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
    fn create(mutator: &mut MutatorRef<impl GcBase>, capacity: usize) -> Gc<Self> {
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
