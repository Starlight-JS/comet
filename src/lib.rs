#![feature(const_type_id)]
use std::mem::size_of;

use gc_info_table::GCInfo;
/// Just like C's offsetof.
///
/// The magic number 0x4000 is insignificant. We use it to avoid using NULL, since
/// NULL can cause compiler problems, especially in cases of multiple inheritance.
#[macro_export]
macro_rules! offsetof {
    ($name : ident . $($field: ident).*) => {
        unsafe {
            let uninit = std::mem::transmute::<_,*const $name>(0x4000usize);
            let fref = &(&*uninit).$($field).*;
            let faddr = fref as *const _ as usize;
            faddr - 0x4000
        }
    }
}

macro_rules! as_atomic {
    ($value: expr;$t: ident) => {
        unsafe { core::mem::transmute::<_, &'_ core::sync::atomic::$t>($value as *const _) }
    };
}

macro_rules! logln_if {
    ($cond: expr, $($t:tt)*) => {
        if $cond {
            println!($($t)*);
        }
    };
}
pub mod block;
pub mod block_allocator;
pub mod gc_info_table;
pub mod gcref;
pub mod global_heap;
pub mod header;
pub mod internal;
pub mod local_heap;
pub mod mmap;
pub mod safepoint;
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
        _gc_info_table_mem: &'static mut [u8; size_of::<GCInfo>() * (1 << 14)],
    ) {
    }
}
