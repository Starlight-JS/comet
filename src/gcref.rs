use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use crate::{
    header::HeapObjectHeader,
    internal::{finalize_trait::FinalizeTrait, gc_info::GCInfoTrait, trace_trait::TraceTrait},
};

#[repr(C)]
pub struct GcRef<T> {
    pub(crate) raw: UntypedGcRef,
    pub(crate) marker: PhantomData<T>,
}

impl<T> GcRef<T> {
    pub fn downcast(self) -> UntypedGcRef {
        self.raw
    }
}

impl<T> Deref for GcRef<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.raw.get().cast::<T>() }
    }
}
impl<T> DerefMut for GcRef<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.raw.get().cast::<T>() }
    }
}
#[repr(C)]
pub struct UntypedGcRef {
    pub(crate) header: NonNull<HeapObjectHeader>,
}

impl UntypedGcRef {
    pub fn get(&self) -> *mut u8 {
        unsafe { (*self.header.as_ptr()).payload() as _ }
    }
    pub fn cast<T: GCInfoTrait<T> + TraceTrait + FinalizeTrait<T> + 'static>(
        self,
    ) -> Option<GcRef<T>> {
        unsafe {
            let header = &*self.header.as_ptr();
            if header.get_gc_info_index() == T::index() {
                return Some(GcRef {
                    raw: self,
                    marker: PhantomData,
                });
            } else {
                None
            }
        }
    }

    pub unsafe fn cast_unchecked<T: GCInfoTrait<T> + TraceTrait + FinalizeTrait<T> + 'static>(
        self,
    ) -> GcRef<T> {
        self.cast::<T>()
            .unwrap_or_else(|| unsafe { core::hint::unreachable_unchecked() })
    }
}
