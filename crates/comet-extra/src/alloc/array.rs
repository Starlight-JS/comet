use std::{
    mem::size_of,
    ops::{Deref, DerefMut},
};

use comet::letroot;

use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::{AllocationSpace, GcBase},
    mutator::MutatorRef,
};

/// GC allocated fixed-size array. It is just like `Box<[T]>` but allocated on GC heap.
#[repr(C, align(8))]
pub struct Array<T: Trace + 'static> {
    pub(crate) length: u32,
    pub(crate) is_inited: bool,
    values: [T; 0],
}

impl<T: Trace + 'static> Array<T> {
    pub fn new_with_default<H: GcBase>(mutator: &mut MutatorRef<H>, len: usize) -> Gc<Array<T>, H>
    where
        T: Default,
    {
        let mut this = mutator.allocate(
            Self {
                length: len as _,
                is_inited: false,
                values: [],
            },
            AllocationSpace::New,
        );
        for i in 0..len {
            this[i] = T::default();
        }
        this.is_inited = true;
        mutator.write_barrier(this.to_dyn());
        this
    }
    pub fn from_slice<H: GcBase, const N: usize>(
        mutator: &mut MutatorRef<H>,
        slice: [T; N],
    ) -> Gc<Self, H> {
        let stack = mutator.shadow_stack();
        letroot!(init = stack, Some(slice));
        let mut this = mutator.allocate(
            Self {
                length: init.as_ref().unwrap().len() as _,
                is_inited: false,
                values: [],
            },
            AllocationSpace::New,
        );
        unsafe {
            std::ptr::copy_nonoverlapping(
                init.as_ref().unwrap().as_ptr(),
                this.data_mut(),
                this.length as _,
            );
        }
        std::mem::forget(init.take().unwrap());
        this.is_inited = true;

        this
    }
    pub fn data(&self) -> *const T {
        self.values.as_ptr()
    }

    pub fn data_mut(&mut self) -> *mut T {
        self.values.as_mut_ptr()
    }

    pub fn len(&self) -> usize {
        self.length as _
    }

    pub fn at(&self, index: usize) -> &T {
        unsafe { &*self.data().add(index) }
    }

    pub fn at_mut(&mut self, index: usize) -> &mut T {
        unsafe { &mut *self.data_mut().add(index) }
    }

    pub fn as_slice(&self) -> &[T] {
        self
    }

    pub fn as_slice_mut(&mut self) -> &mut [T] {
        self
    }
}

impl<T: Trace> Deref for Array<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.data(), self.len()) }
    }
}

impl<T: Trace> DerefMut for Array<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.data_mut(), self.len()) }
    }
}

impl<T: Trace + std::fmt::Debug> std::fmt::Debug for Array<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Array(")?;
        for i in 0..self.len() {
            write!(f, "{:?}", self.at(i))?;
            if i != self.len() - 1 {
                write!(f, ",")?
            }
        }
        write!(f, ")")
    }
}

unsafe impl<T: Trace> Trace for Array<T> {
    fn trace(&mut self, vis: &mut dyn crate::api::Visitor) {
        if !self.is_inited {
            return;
        }
        let mut cursor = self.data_mut();
        unsafe {
            let end = cursor.add(self.length as _);
            while cursor < end {
                (&mut *cursor).trace(vis);
                cursor = cursor.add(1);
            }
        }
    }
}

unsafe impl<T: Trace> Finalize for Array<T> {}
impl<T: Trace> Collectable for Array<T> {
    fn allocation_size(&self) -> usize {
        self.length as usize * size_of::<T>() + size_of::<Self>()
    }
}
