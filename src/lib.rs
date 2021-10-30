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

/// Configuration for heap constructor.
#[repr(C)]
pub struct Config {
    /// How fast heap threshold should grow
    pub heap_growth_factor: f64,
    /// Heap size. It is heap size only for Immix block space. LargeObjectSpace will allocate until System OOMs
    pub heap_size: usize,
    /// Maximum heap size before first GC
    pub max_heap_size: usize,
    /// Maximum eden heap size before first GC (does not matter atm)
    pub max_eden_size: usize,
    /// Enables verbose printing
    pub verbose: bool,
    /// Enable generational GC (does not work atm)
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
            heap_size: 1 * 1024 * 1024 * 1024,
        }
    }
}

/// Returns GC allocation size of object. 
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

pub mod c_api {

    use std::ptr::{null_mut, NonNull};

    use crate::{
        gc_info_table::{GCInfo, GC_TABLE},
        gcref::{UntypedGcRef, WeakGcRef},
        header::HeapObjectHeader,
        heap::Heap,
        internal::gc_info::GCInfoIndex,
        visitor::Visitor,
        Config, GCPlatform,
    };

    #[no_mangle]
    pub extern "C" fn comet_gc_size(ptr: *const HeapObjectHeader) -> usize {
        super::gc_size(ptr)
    }
    #[no_mangle]
    pub extern "C" fn comet_default_config() -> Config {
        Config::default()
    }
    #[no_mangle]
    pub extern "C" fn comet_init() {
        GCPlatform::initialize();
    }
    #[no_mangle]
    pub extern "C" fn comet_heap_create(config: Config) -> *mut Heap {
        Box::into_raw(Heap::new(config))
    }
    /// Free comet heap
    #[no_mangle]
    pub extern "C" fn comet_heap_free(heap: *mut Heap) {
        unsafe {
            Box::from_raw(heap);
        }
    }

    /// Add GC constraint to the Comet Heap. Each constraint is executed when marking starts
    /// to obtain list of root objects.
    #[no_mangle]
    pub extern "C" fn comet_heap_add_constraint(
        heap: *mut Heap,
        data: *mut u8,
        callback: extern "C" fn(*mut u8, *mut Visitor),
    ) {
        unsafe {
            (*heap).add_constraint(move |vis: &mut Visitor| {
                let data = data;
                callback(data, vis as *mut _);
            });
        }
    }

    /// Add core constraints to the heap. This one will setup stack scanning routines.
    #[no_mangle]
    pub extern "C" fn comet_heap_add_core_constraints(heap: *mut Heap) {
        unsafe {
            (*heap).add_core_constraints();
        }
    }

    #[no_mangle]
    pub extern "C" fn comet_heap_collect(heap: *mut Heap) {
        unsafe {
            (*heap).collect_garbage();
        }
    }

    #[no_mangle]
    pub extern "C" fn comet_heap_collect_if_necessary_or_defer(heap: *mut Heap) {
        unsafe {
            (*heap).collect_if_necessary_or_defer();
        }
    }

    #[no_mangle]
    pub extern "C" fn comet_heap_allocate_weak(
        heap: *mut Heap,
        object: *mut HeapObjectHeader,
    ) -> WeakGcRef {
        unsafe {
            (*heap).allocate_weak(UntypedGcRef {
                header: NonNull::new_unchecked(object),
            })
        }
    }

    /// Allocates memory and returns pointer. NULL is returned if no memory is available.
    #[no_mangle]
    pub extern "C" fn comet_heap_allocate(
        heap: *mut Heap,
        size: usize,
        index: GCInfoIndex,
    ) -> *mut HeapObjectHeader {
        unsafe {
            match (*heap).allocate_raw(size, index) {
                Some(mem) => mem.header.as_ptr(),
                None => null_mut(),
            }
        }
    }

    /// Allocates memory and returns pointer. When no memory is left process is aborted.
    #[no_mangle]
    pub extern "C" fn comet_heap_allocate_or_fail(
        heap: *mut Heap,
        size: usize,
        index: GCInfoIndex,
    ) -> *mut HeapObjectHeader {
        unsafe { (*heap).allocate_raw_or_fail(size, index).header.as_ptr() }
    }

    /// Upgrade weak ref. If it is still alive then pointer is returned otherwise NULL is returned.
    #[no_mangle]
    pub extern "C" fn comet_weak_upgrade(weak: WeakGcRef) -> *mut HeapObjectHeader {
        match weak.upgrade() {
            Some(ptr) => ptr.header.as_ptr(),
            None => null_mut(),
        }
    }

    #[no_mangle]
    pub extern "C" fn comet_trace(vis: *mut Visitor, ptr: *mut HeapObjectHeader) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            (*vis).trace_untyped(UntypedGcRef {
                header: NonNull::new_unchecked(ptr),
            })
        }
    }

    #[no_mangle]
    pub extern "C" fn comet_trace_conservatively(
        vis: *mut Visitor,
        from: *const u8,
        to: *const u8,
    ) {
        unsafe { (*vis).trace_conservatively(from, to) }
    }

    #[no_mangle]
    pub extern "C" fn comet_add_gc_info(info: GCInfo) -> GCInfoIndex {
        unsafe { GC_TABLE.add_gc_info(info) }
    }

    #[no_mangle]
    pub extern "C" fn comet_get_gc_info(index: GCInfoIndex) -> *mut GCInfo {
        unsafe { GC_TABLE.get_gc_info_mut(index) as *mut _ }
    }
}
