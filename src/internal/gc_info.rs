use std::{any::TypeId, sync::atomic::AtomicU16};

use crate::gc_info_table::{GCInfo, GC_TABLE};

use super::{finalize_trait::FinalizeTrait, trace_trait::TraceTrait};

pub type GCInfoIndex = u16;

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
                },
            )
        }
    }
}
