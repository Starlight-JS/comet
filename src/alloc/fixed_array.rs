use std::{
    mem::size_of,
    ops::{Deref, DerefMut},
};

use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    base::GcBase,
};

#[repr(C, align(16))]
pub struct FixedArray<T: Trace> {
    length: usize,
    data_start: [T; 0],
}

impl<T: Trace + 'static> Collectable for FixedArray<T> {
    fn allocation_size(&self) -> usize {
        size_of::<Self>() + (self.length * size_of::<T>())
    }
}

struct TraceSlice<T> {
    start: *mut T,
    len: usize,
}

unsafe impl<T: Trace> Trace for TraceSlice<T> {
    fn trace(&mut self, _vis: &mut dyn crate::api::Visitor) {
        for i in 0..self.len {
            unsafe {
                (&mut *self.start.add(i)).trace(_vis);
            }
        }
    }
}
impl<T: Trace + 'static> FixedArray<T> {
    pub fn new(heap: &mut impl GcBase, init: &mut [T]) -> Gc<Self> {
        let alloc_size = init.len() * size_of::<T>() + size_of::<Self>();
        let mut result = heap.allocate_raw::<Self>(alloc_size);
        while let None = result {
            let mut keep = [&mut TraceSlice {
                start: init.as_mut_ptr(),
                len: init.len(),
            } as &mut dyn Trace];
            heap.collect(&mut keep);
            result = heap.allocate_raw(alloc_size);
        }
        unsafe {
            let mut this = result.unwrap().assume_init();
            this.length = init.len();
            std::ptr::copy_nonoverlapping(init.as_ptr(), this.data_mut(), init.len());
            this
        }
    }
    pub fn data(&self) -> *const T {
        self.data_start.as_ptr()
    }

    pub fn data_mut(&mut self) -> *mut T {
        self.data_start.as_mut_ptr()
    }

    pub fn len(&self) -> usize {
        self.length
    }
}
impl<T: Trace + 'static> AsRef<[T]> for FixedArray<T> {
    fn as_ref(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.data(), self.len()) }
    }
}

impl<T: Trace + 'static> AsMut<[T]> for FixedArray<T> {
    fn as_mut(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.data_mut(), self.len()) }
    }
}
impl<T: Trace + 'static> Deref for FixedArray<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T: Trace + 'static> DerefMut for FixedArray<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}
unsafe impl<T: Trace + 'static> Trace for FixedArray<T> {
    fn trace(&mut self, vis: &mut dyn crate::api::Visitor) {
        for i in 0..self.length {
            unsafe {
                (&mut *self.data_mut().add(i)).trace(vis);
            }
        }
    }
}

unsafe impl<T: Trace> Finalize for FixedArray<T> {}
