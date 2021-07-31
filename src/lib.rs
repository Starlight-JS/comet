#![feature(const_type_id)]
use std::mem::size_of;

use gc_info_table::GCInfo;

pub mod gc_info_table;
pub mod gcref;
pub mod header;
pub mod internal;
pub mod mmap;
pub mod visitor;
pub struct GCPlatform;

impl GCPlatform {
    /// Initializes global state for GC.
    pub fn initialize() {
        #[cfg(target_family = "wasm")]
        {
            panic!("Invoke GCPlatform::initialize_wasm on WASM!");
        }
        unsafe {
            gc_info_table::GCInfoTable::init(None);
        }
    }

    pub unsafe fn initialize_wasm(
        gc_info_table_mem: &'static mut [u8; size_of::<GCInfo>() * (1 << 14)],
    ) {
    }
}
