use std::{any::TypeId, sync::atomic::AtomicU16};

use crate::gc_info_table::{GCInfo, GC_TABLE};

use super::{finalize_trait::FinalizeTrait, trace_trait::TraceTrait};

pub trait GCInfoTrait<T: TraceTrait + FinalizeTrait<T> + Sized + 'static> {
    const REGISTERED_INDEX: AtomicU16;
    fn index() -> GCInfoIndex;
}

impl<T: TraceTrait + FinalizeTrait<T> + Sized + 'static> GCInfoTrait<T> for T {
    const REGISTERED_INDEX: AtomicU16 = AtomicU16::new(0);
    fn index() -> GCInfoIndex {
        unsafe {
            GC_TABLE.add_gc_info_type_id(
                TypeId::of::<T>(),
                GCInfo {
                    finalize: <T as FinalizeTrait<T>>::CALLBACK,
                    trace: <T as TraceTrait>::trace_,
                    vtable: 0,
                },
            )
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct GCInfoIndex(pub(crate) u16);

impl GCInfoIndex {
    pub fn get(self) -> GCInfo {
        unsafe { GC_TABLE.get_gc_info(self) }
    }
    /// Obtain mutable reference to GCInfo.
    ///
    /// # Safety
    /// Unsafe since modifying GCInfo is unsafe and might break GC.
    ///
    pub unsafe fn get_mut(self) -> &'static mut GCInfo {
        GC_TABLE.get_gc_info_mut(self)
    }
}
