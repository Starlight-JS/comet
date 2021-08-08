#![feature(const_type_id)]
use std::mem::size_of;

use gc_info_table::GCInfo;
use header::HeapObjectHeader;
use large_space::PreciseAllocation;
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
/// rounds the given value `val` up to the nearest multiple
/// of `align`
pub fn align(value: u32, align: u32) -> u32 {
    if align == 0 {
        return value;
    }

    ((value + align - 1) / align) * align
}
macro_rules! log_if {
    ($cond: expr, $($t:tt)*) => {
        if $cond {
            print!($($t)*);
        }
    };
}
pub mod allocation_config;
pub mod allocator;
pub mod block;
pub mod block_allocator;
pub mod gc_info_table;
pub mod gcref;
pub mod global_allocator;
pub mod globals;
pub mod header;
pub mod heap;
pub mod internal;
pub mod large_space;
pub mod marking;
pub mod mmap;
pub mod task_scheduler;
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
    pub max_heap_size: usize,
    pub max_eden_size: usize,
    pub verbose: bool,
    pub generational: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            generational: false,
            verbose: false,
            max_eden_size: 64 * 1024,
            max_heap_size: 256 * 1024,
            heap_growth_factor: 1.5,
            heap_growth_threshold: 0.78,
            large_heap_growth_factor: 1.5,
            large_heap_growth_threshold: 0.9,
            dump_size_classes: false,
            size_class_progression: 1.4,
            heap_size: 1 * 1024 * 1024 * 1024,
        }
    }
}

pub fn gc_size(ptr: *const HeapObjectHeader) -> usize {
    unsafe {
        let size = (*ptr).get_size();
        if size == 0 {
            (*PreciseAllocation::from_cell(ptr as _)).cell_size()
        } else {
            size
        }
    }
}
