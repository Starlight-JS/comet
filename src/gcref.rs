use std::{
    collections::HashMap,
    fmt::{self},
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

#[derive(Clone, Copy)]
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

impl fmt::Debug for UntypedGcRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UntypedGcRef({:p})", self.header)
    }
}
impl fmt::Pointer for UntypedGcRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UntypedGcRef({:p})", self.header)
    }
}
impl<T> std::fmt::Pointer for GcRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:p}", self.raw)
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for GcRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", **self)
    }
}
impl<T: std::fmt::Display> std::fmt::Display for GcRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", **self)
    }
}
pub struct WeakSlot {
    pub(crate) value: Option<UntypedGcRef>,
}

impl FinalizeTrait<WeakSlot> for WeakSlot {}
impl TraceTrait for WeakSlot {}

#[repr(transparent)]
pub struct WeakGcRef {
    pub(crate) slot: GcRef<WeakSlot>,
}

impl WeakGcRef {
    pub fn upgrade(&self) -> Option<UntypedGcRef> {
        self.slot.value
    }
}
impl<T> Copy for GcRef<T> {}

impl<T> Clone for GcRef<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: TraceTrait> TraceTrait for GcRef<T> {
    fn trace(&self, vis: &mut crate::visitor::Visitor) {
        vis.trace_gcref(*self);
    }
}

impl TraceTrait for UntypedGcRef {
    fn trace(&self, vis: &mut crate::visitor::Visitor) {
        vis.trace_untyped(*self);
    }
}

macro_rules! impl_prim {
    ($($t:ty)*) => {
        $(
            impl FinalizeTrait<$t> for $t {

            }
            impl TraceTrait for $t {}
        )*
    };
}

impl_prim! (
    bool f32 f64
    u8 u16 u32 u64 u128
    i8 i16 i32 i64 i128
    String std::fs::File
    std::path::PathBuf
);

impl<T> FinalizeTrait<Vec<T>> for Vec<T> {}
impl<T: TraceTrait> TraceTrait for Vec<T> {
    fn trace(&self, vis: &mut crate::visitor::Visitor) {
        for elem in self.iter() {
            vis.trace_ref(elem);
        }
    }
}

impl<K, V> FinalizeTrait<HashMap<K, V>> for HashMap<K, V> {}
impl<K: TraceTrait, V: TraceTrait> TraceTrait for HashMap<K, V> {
    fn trace(&self, vis: &mut crate::visitor::Visitor) {
        for (k, v) in self.iter() {
            k.trace(vis);
            v.trace(vis);
        }
    }
}

impl<T> FinalizeTrait<Option<T>> for Option<T> {}

impl<T: TraceTrait> TraceTrait for Option<T> {
    fn trace(&self, vis: &mut crate::visitor::Visitor) {
        match self {
            Some(elem) => elem.trace(vis),
            _ => (),
        }
    }
}

impl<T, E> FinalizeTrait<Result<T, E>> for Result<T, E> {}
impl<T: TraceTrait, E: TraceTrait> TraceTrait for Result<T, E> {
    fn trace(&self, vis: &mut crate::visitor::Visitor) {
        match self {
            Ok(x) => x.trace(vis),
            Err(x) => x.trace(vis),
        }
    }
}

impl PartialEq for UntypedGcRef {
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header
    }
}

impl Eq for UntypedGcRef {}

impl<T> PartialEq for GcRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T> Eq for GcRef<T> {}
