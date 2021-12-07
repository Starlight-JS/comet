use std::{
    marker::PhantomData,
    mem::size_of,
    ops::{Index, IndexMut},
};

use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    base::GcBase,
};

/// Vector for heap allocated values.
#[repr(C)]
pub struct Vector<T: Collectable + Sized, H: GcBase = crate::Heap> {
    length: u32,
    capacity: u32,
    marker: PhantomData<H>,
    data_start: [T; 0],
}

impl<T: Collectable + Sized, H: GcBase + 'static> Vector<T, H> {
    pub fn new(heap: &mut H) -> Gc<Self> {
        Self::with_capacity(heap, 0)
    }
    pub fn with_capacity(heap: &mut H, capacity: usize) -> Gc<Self> {
        let this = heap.allocate(Self {
            length: 0,
            capacity: capacity as _,
            marker: Default::default(),
            data_start: [],
        });

        this
    }

    pub fn data(&self) -> *const T {
        self.data_start.as_ptr()
    }

    pub fn data_mut(&mut self) -> *mut T {
        self.data_start.as_mut_ptr()
    }

    pub fn len(&self) -> usize {
        self.length as _
    }

    pub fn capacity(&self) -> usize {
        self.capacity as _
    }
}

impl<T: Collectable + Sized, H: GcBase + 'static> Gc<Vector<T, H>> {
    #[inline]
    pub fn push_back(&mut self, heap: &mut H, value: T) -> Self {
        if self.length == self.capacity {
            return self.push_back_slow(heap, value);
        }
        unsafe {
            self.data_mut().add(self.length as _).write(value);
            self.length += 1;
            *self
        }
    }
    #[cold]
    fn push_back_slow(&mut self, heap: &mut H, value: T) -> Self {
        // protect value so if value is Gc pointer then it is traced.
        letroot!(value = heap.shadow_stack(), Some(value));
        self.realloc(heap, 0);
        unsafe {
            self.data_mut()
                .add(self.length as _)
                .write(value.take().unwrap());
            self.length += 1;
            *self
        }
    }
    #[inline]
    pub fn pop_back(&mut self) -> Option<T> {
        if self.length == 0 {
            return None;
        }
        unsafe {
            self.length -= 1;
            let value = self.data().add(self.length as _).read();
            Some(value)
        }
    }

    // allocates new vector in GC heap. Updates `self` with new vector.
    #[inline(never)]
    fn realloc(&mut self, heap: &mut H, size: usize) -> Self {
        letroot!(this = heap.shadow_stack(), *self);
        let capacity = next_capacity::<T>(this.capacity as _).max(size);
        let mut new_self = heap.allocate(Vector::<T, H> {
            length: 0,
            capacity: capacity as _,
            marker: Default::default(),
            data_start: [],
        });

        unsafe {
            core::ptr::copy_nonoverlapping(this.data(), new_self.data_mut(), this.length as _);
        }

        new_self.length = self.length as _;
        // set length to zero in case `needs_drop::<T>()` returns true and this vector will be finalized.
        this.length = 0;
        *self = new_self;
        new_self
    }
    #[inline]
    pub fn insert(&mut self, heap: &mut H, index: usize, value: T) -> Self {
        let len = self.len();
        letroot!(value = heap.shadow_stack(), Some(value));

        if index > len {
            panic!(
                "insertion index (is {}) should be <= len (is {})",
                index, len
            );
        }

        if len == self.capacity() {
            *self = self.reserve(heap);
        }
        unsafe {
            let p = self.data_mut().add(index);
            std::ptr::copy(p, p.add(1), len - index);
            std::ptr::write(p, value.take().unwrap());
            self.length = (len + 1) as u32;
        }
        *self
    }

    pub fn reserve(&mut self, heap: &mut H) -> Self {
        *self = self.realloc(heap, self.capacity() + 1);
        *self
    }
}

impl<T: Collectable + Sized, H: GcBase + 'static> Index<usize> for Vector<T, H> {
    type Output = T;
    fn index(&self, index: usize) -> &Self::Output {
        assert!(
            index < self.len(),
            "Out of bounds {}, len {}",
            index,
            self.len()
        );
        unsafe { &*self.data().add(index) }
    }
}
impl<T: Collectable + Sized, H: GcBase + 'static> IndexMut<usize> for Vector<T, H> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        assert!(
            index < self.len(),
            "Out of bounds {}, len {}",
            index,
            self.len()
        );
        unsafe { &mut *self.data_mut().add(index) }
    }
}
unsafe impl<T: Collectable + Sized, H: GcBase + 'static> Trace for Vector<T, H> {
    fn trace(&mut self, vis: &mut dyn crate::api::Visitor) {
        for i in 0..self.length {
            unsafe {
                (*self.data_mut().add(i as _)).trace(vis);
            }
        }
    }
}

unsafe impl<T: Collectable + Sized, H: GcBase> Finalize for Vector<T, H> {}

impl<T: Collectable + Sized, H: GcBase + 'static> Collectable for Vector<T, H> {
    fn allocation_size(&self) -> usize {
        self.capacity as usize * size_of::<T>() + size_of::<Self>()
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
