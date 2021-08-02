#![feature(const_type_id)]
use std::mem::size_of;

use gc_info_table::GCInfo;
use internal::BLOCK_SIZE;
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
pub mod allocation_config;
pub mod block;
pub mod block_allocator;
pub mod gc_info_table;
pub mod gcref;
pub mod global_allocator;
pub mod header;
pub mod heap;
pub mod internal;
pub mod large_space;
pub mod local_allocator;
pub mod local_heap;
pub mod marking;
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

pub struct Config {
    pub heap_growth_factor: f64,
    pub heap_growth_threshold: f64,
    pub large_heap_growth_factor: f64,
    pub large_heap_growth_threshold: f64,
    pub dump_size_classes: bool,
    pub size_class_progression: f64,
    pub heap_size: usize,
    pub large_threshold: usize,
    pub block_threshold: usize,
    pub verbose: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            verbose: false,
            heap_growth_factor: 1.5,
            heap_growth_threshold: 0.78,
            large_heap_growth_factor: 1.5,
            large_heap_growth_threshold: 0.9,
            dump_size_classes: false,
            size_class_progression: 1.4,
            heap_size: 1 * 1024 * 1024 * 1024,
            large_threshold: 4 * 1024 * 1024, // 4MB
            block_threshold: (4 * 1024 * 1024) / BLOCK_SIZE,
        }
    }
}
