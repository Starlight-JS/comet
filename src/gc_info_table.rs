use std::{
    any::TypeId,
    collections::hash_map::DefaultHasher,
    hash::Hash,
    hash::Hasher,
    mem::{size_of, ManuallyDrop, MaybeUninit},
    ptr::null_mut,
    sync::atomic::AtomicU16,
};

use crate::internal::{
    finalize_trait::FinalizationCallback, gc_info::GCInfoIndex, trace_trait::TraceCallback,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::mmap::Mmap;

/// GCInfo contains metadata for objects.
pub struct GCInfo {
    pub finalize: Option<FinalizationCallback>,
    pub trace: TraceCallback,
}

pub struct GCInfoTable {
    #[cfg(not(wasm))]
    map: Mmap,
    table: *mut GCInfo,
    type_id_map: MaybeUninit<Vec<AtomicU16>>,
    current_index: AtomicU16,
}

pub(crate) static mut GC_TABLE: GCInfoTable = GCInfoTable {
    table: null_mut(),
    current_index: AtomicU16::new(1),
    type_id_map: MaybeUninit::uninit(),
    #[cfg(not(wasm))]
    map: Mmap::uninit(),
};

impl GCInfoTable {
    /// At maximum [`MAX_INDEX - 1`](GCInfoTable::MAX_INDEX) indices are supported.
    ///
    /// We assume that 14 bits are enough to represent all possible types.
    pub const MAX_INDEX: u16 = 1 << 14;
    /// Minimum index returned. Values smaller [`MIN_INDEX`](GCInfoTable::MIN_INDEX) may be used as
    /// sentinels.
    pub const MIN_INDEX: u16 = 1;

    pub const INITIAL_WANTED_LIMIT: u16 = 512;

    pub(crate) unsafe fn init(mem: Option<&'static mut [u8]>) {
        #[cfg(wasm)]
        {
            GC_TABLE.table = mem.unwrap().as_mut_ptr();
        }
        #[cfg(not(wasm))]
        {
            let _ = mem;
            let map = Mmap::new(Self::MAX_INDEX as usize * size_of::<GCInfo>());
            GC_TABLE.map = map;
            GC_TABLE.table = GC_TABLE.map.start().cast();
        }
        let mut v = ManuallyDrop::new(vec![0u16; Self::MAX_INDEX as usize]);
        *GC_TABLE.type_id_map.as_mut_ptr() =
            Vec::from_raw_parts(v.as_mut_ptr().cast::<AtomicU16>(), v.len(), v.capacity());
    }
    pub(crate) fn add_gc_info_type_id(&mut self, type_id: TypeId, info: GCInfo) -> GCInfoIndex {
        unsafe {
            let mut hasher = DefaultHasher::default();
            type_id.hash(&mut hasher);
            let key = hasher.finish();
            let table_idx = key % (*self.type_id_map.as_ptr()).len() as u64;
            let index = &(*self.type_id_map.as_ptr())[table_idx as usize];
            let index_ = index.load(std::sync::atomic::Ordering::Acquire);
            if index_ != 0 {
                return GCInfoIndex(index_);
            }
            let index_ = self.add_gc_info(info);
            index.store(index_.0, std::sync::atomic::Ordering::Release);
            index_
        }
    }

    pub unsafe fn add_gc_info(&mut self, info: GCInfo) -> GCInfoIndex {
        let index = self
            .current_index
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if index >= Self::MAX_INDEX {
            panic!("GCInfoTable memory exhausted");
        }

        self.table.add(index as _).write(info);

        GCInfoIndex(index)
    }

    pub unsafe fn get_gc_info(&self, index: GCInfoIndex) -> GCInfo {
        self.table.add(index.0 as _).read()
    }

    pub unsafe fn get_gc_info_mut(&mut self, index: GCInfoIndex) -> &mut GCInfo {
        &mut *self.table.add(index.0 as _)
    }
}
