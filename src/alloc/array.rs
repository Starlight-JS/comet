use std::mem::size_of;

use crate::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::GcBase,
    mutator::{Mutator, MutatorRef},
};

#[repr(C, align(8))]
pub struct Array<T: Trace + 'static> {
    pub(crate) length: u32,
    pub(crate) is_inited: bool,
    values: [T; 0],
}

impl<T: Trace + 'static> Array<T> {
    pub fn from_slice<const N: usize>(
        mutator: &mut MutatorRef<impl GcBase>,
        slice: [T; N],
    ) -> Gc<Self> {
        let stack = mutator.shadow_stack();
        letroot!(init = stack, Some(slice));
        let mut this = mutator.allocate(Self {
            length: init.as_ref().unwrap().len() as _,
            is_inited: false,
            values: [],
        });
        unsafe {
            std::ptr::copy_nonoverlapping(
                init.as_ref().unwrap().as_ptr(),
                this.data_mut(),
                this.length as _,
            );
        }
        std::mem::forget(init.take().unwrap());
        this.is_inited = true;

        todo!()
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

pub trait ArrayMake<T>: Collectable {
    fn make(mutator: &Mutator<impl GcBase>, length: usize, init: T) -> Gc<Self>;
}
