use crate::{
    allocator::Allocator,
    block::Block,
    gcref::{GcRef, UntypedGcRef, WeakGcRef, WeakSlot},
    global_allocator::{round_up, GlobalAllocator},
    globals::{LARGE_CUTOFF, MEDIUM_CUTOFF},
    header::HeapObjectHeader,
    internal::{
        block_list::BlockList,
        collection_barrier::CollectionBarrier,
        gc_info::{GCInfoIndex, GCInfoTrait},
        stack_bounds::StackBounds,
    },
    large_space::PreciseAllocation,
    marking::SynchronousMarking,
    visitor::Visitor,
    Config,
};
use std::{
    mem::{size_of, swap},
    ptr::{null_mut, NonNull},
    sync::atomic::AtomicUsize,
};

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
pub enum CollectionScope {
    Full,
    Eden,
}

pub trait MarkingConstraint {
    fn execute(&mut self, vis: &mut Visitor);
}

impl<T: FnMut(&mut Visitor)> MarkingConstraint for T {
    fn execute(&mut self, vis: &mut Visitor) {
        self(vis);
    }
}
thread_local! {static BOUNDS: StackBounds = StackBounds::current_thread_stack_bounds();}

#[repr(C)]
#[allow(dead_code)]
pub struct Heap {
    pub(crate) global: GlobalAllocator,
    pub(crate) gc_prepare_stw_callback: Option<Box<dyn FnMut()>>,
    collection_barrier: CollectionBarrier,
    config: Config,

    pub(crate) constraints: Vec<Box<dyn MarkingConstraint>>,

    generational_gc: bool,
    size_after_last_collect: usize,
    size_after_last_full_collect: usize,
    size_before_last_full_collect: usize,
    size_after_last_eden_collect: usize,
    size_before_last_eden_collect: usize,
    pub(crate) defers: AtomicUsize,
    pub(crate) bytes_allocated_this_cycle: usize,
    pub(crate) max_eden_size: usize,
    max_heap_size: usize,
    total_bytes_visited: usize,
    total_bytes_visited_this_cycle: usize,
    increment_balance: f64,
    should_do_full_collection: bool,
    collection_scope: Option<CollectionScope>,
    last_collection_scope: Option<CollectionScope>,
    pub(crate) weak_references: Vec<GcRef<WeakSlot>>,
}

impl Heap {
    /// Walks all cells allocated in the heap.
    pub fn for_each_cell(
        &mut self,

        mut callback: impl FnMut(*mut HeapObjectHeader),
        mut weak_refs: impl FnMut(GcRef<WeakSlot>),
    ) {
        unsafe {
            for weak in self.weak_references.iter() {
                weak_refs(*weak);
            }

            self.global.large_space.allocations.iter().for_each(|cell| {
                callback((**cell).cell());
            });
        }
    }

    pub fn add_constraint(&mut self, constraint: impl MarkingConstraint + 'static) {
        self.constraints.push(Box::new(constraint));
    }

    pub fn add_core_constraints(&mut self) {
        self.add_constraint(|visitor: &mut Visitor| unsafe {
            let heap = &mut *visitor.heap();

            heap.global.large_space.prepare_for_conservative_scan();

            let mut from = BOUNDS.with(|b| b.origin);
            let mut to = approximate_stack_pointer();
            if from > to {
                swap(&mut from, &mut to);
            }
            visitor.trace_conservatively(from, to)
        });
    }

    pub fn new(config: Config) -> Box<Self> {
        let mut this = Box::new(Self {
            constraints: Vec::new(),
            generational_gc: config.generational,

            defers: AtomicUsize::new(0),
            global: GlobalAllocator::new(&config),
            gc_prepare_stw_callback: None,

            collection_barrier: CollectionBarrier::new(null_mut()),
            weak_references: vec![],

            should_do_full_collection: false,
            size_after_last_collect: 0,
            size_after_last_eden_collect: 0,
            size_after_last_full_collect: 0,
            size_before_last_eden_collect: 0,
            size_before_last_full_collect: 0,
            max_eden_size: config.max_eden_size,
            max_heap_size: config.max_heap_size,
            collection_scope: None,
            last_collection_scope: None,
            total_bytes_visited: 0,
            total_bytes_visited_this_cycle: 0,
            increment_balance: 0.0,
            bytes_allocated_this_cycle: 0,
            config,
        });

        this.global.normal_allocator.line_bitmap = &this.global.line_bitmap;
        this.global.overflow_allocator.line_bitmap = &this.global.line_bitmap;
        this
    }

    pub(crate) unsafe fn sweep(&mut self, blocks: BlockList) {
        // Sweep global allocator
        self.global.sweep(blocks);
    }

    pub fn collect_garbage(&mut self) {
        if self.defers.load(atomic::Ordering::SeqCst) > 0 {
            return;
        }
        self.perform_garbage_collection();
    }
    #[allow(dead_code)]
    pub fn collect_if_necessary_or_defer(&mut self) {
        if self.defers.load(atomic::Ordering::Relaxed) > 0 {
            return;
        } else {
            let bytes_allowed = self.max_eden_size;

            if self.bytes_allocated_this_cycle >= bytes_allowed {
                self.collect_garbage();
            }
        }
    }

    pub unsafe fn allocate_weak(&mut self, target: UntypedGcRef) -> WeakGcRef {
        let ptr = self.allocate_raw_or_fail(
            size_of::<WeakSlot>() + size_of::<HeapObjectHeader>(),
            WeakSlot::index(),
        );

        ptr.get().cast::<WeakSlot>().write(WeakSlot {
            value: Some(target),
        });
        WeakGcRef {
            slot: ptr.cast_unchecked(),
        }
    }

    pub unsafe fn allocate_raw(&mut self, size: usize, index: GCInfoIndex) -> Option<UntypedGcRef> {
        let size = round_up(size, 8);
        let cell = if size >= LARGE_CUTOFF {
            return self.allocate_large(size, index);
        } else if size < MEDIUM_CUTOFF {
            self.global.normal_allocator.allocate(size)
        } else {
            self.global.overflow_allocator.allocate(size)
        };
        cell.map(|x| {
            (*x).set_size(size);

            self.bytes_allocated_this_cycle += size;
            (*x).set_gc_info(index);
            debug_assert!(
                {
                    let mut scan = x as usize;
                    let end = scan + size;
                    let mut f = true;
                    while scan < end {
                        if self.global.live_bitmap.test(scan as _) {
                            f = false;
                            break;
                        }
                        scan += 8;
                    }
                    f
                },
                "object at {:p} was already allocated!",
                x
            );
            self.global.live_bitmap.set(x as _);
            UntypedGcRef {
                header: NonNull::new_unchecked(x),
            }
        })
    }
    #[cold]
    unsafe fn try_perform_collection_and_allocate_again(
        &mut self,
        gc_info: GCInfoIndex,
        size: usize,
    ) -> UntypedGcRef {
        for _ in 0..3 {
            self.collect_garbage();
            let result = self.allocate_raw(size, gc_info);
            if let Some(result) = result {
                return result;
            }
        }
        eprintln!("Allocation of {} bytes failed: OOM", size);
        std::process::abort();
    }
    pub unsafe fn allocate_raw_or_fail(&mut self, size: usize, index: GCInfoIndex) -> UntypedGcRef {
        let mem = self.allocate_raw(size, index);
        if mem.is_none() {
            return self.try_perform_collection_and_allocate_again(index, size);
        }
        mem.unwrap()
    }

    fn allocate_large(&mut self, size: usize, index: GCInfoIndex) -> Option<UntypedGcRef> {
        unsafe {
            let cell = self.global.large_space.allocate(size);
            self.bytes_allocated_this_cycle += (*PreciseAllocation::from_cell(cell)).cell_size();
            (*cell).set_gc_info(index);
            (*cell).set_size(0);
            Some(UntypedGcRef {
                header: NonNull::new_unchecked(cell),
            })
        }
    }

    fn is_marked(&self, hdr: *const HeapObjectHeader) -> bool {
        unsafe {
            if !(*hdr).is_precise() {
                self.global.mark_bitmap.test(hdr as _)
            } else {
                (*PreciseAllocation::from_cell(hdr as _)).is_marked()
            }
        }
    }

    fn update_weak_references(&self) {
        let weak_refs = &self.weak_references;

        for weak in weak_refs.iter() {
            match weak.value {
                Some(value) if self.is_marked(value.header.as_ptr()) => {
                    continue;
                }
                _ => {
                    let mut weak = *weak;
                    weak.value = None;
                }
            }
        }
    }

    fn reset_weak_references(&mut self) {
        let bitmap = &self.global.mark_bitmap;
        self.weak_references
            .retain(|ref_| bitmap.test(ref_.into_raw() as _));
    }
    fn will_start_collection(&mut self) {
        log_if!(self.config.verbose, " => ");

        self.collection_scope = Some(CollectionScope::Full);
        self.should_do_full_collection = false;
        logln_if!(self.config.verbose, "Collection, ");

        self.size_before_last_full_collect =
            self.size_after_last_collect + self.bytes_allocated_this_cycle;
    }
    fn update_object_counts(&mut self, bytes_visited: usize) {
        self.total_bytes_visited = 0;

        self.total_bytes_visited_this_cycle = bytes_visited;
        self.total_bytes_visited += self.total_bytes_visited_this_cycle;
    }
    fn update_allocation_limits(&mut self) {
        // Calculate our current heap size threshold for the purpose of figuring out when we should
        // run another collection.
        let current_heap_size = self.total_bytes_visited;

        // To avoid pathological GC churn in very small and very large heaps, we set
        // the new allocation limit based on the current size of the heap, with a
        // fixed minimum.
        self.max_heap_size =
            (self.config.heap_growth_factor * current_heap_size as f64).ceil() as _;
        self.max_eden_size = self.max_heap_size - current_heap_size;
        self.size_after_last_full_collect = current_heap_size;

        self.size_after_last_collect = current_heap_size;
        self.bytes_allocated_this_cycle = 0;
        logln_if!(
            self.config.verbose,
            " => {}\n => threshold: {}kb",
            current_heap_size,
            self.max_heap_size as f64 / 1024.
        );
    }
    pub(crate) fn test_and_set_marked(&self, hdr: *const HeapObjectHeader) -> bool {
        unsafe {
            if self.global.block_allocator.is_in_space(hdr as _) {
                self.global.mark_bitmap.set(hdr as _)
            } else {
                debug_assert!(!self.global.large_space.contains(hdr as _).is_null());
                (*PreciseAllocation::from_cell(hdr as _)).test_and_set_marked()
            }
        }
    }
    #[inline(never)]
    pub(crate) fn perform_garbage_collection(&mut self) {
        unsafe {
            self.will_start_collection();
            self.global
                .prepare_for_marking(self.collection_scope == Some(CollectionScope::Eden));
            let (live, blocks) = {
                let blocks = self.global.begin_marking();
                let mut marking = SynchronousMarking::new(self);
                (marking.run(), blocks)
            };
            self.update_weak_references();
            self.reset_weak_references();
            self.sweep(blocks);
            self.update_object_counts(live);

            self.global
                .large_space
                .prepare_for_allocation(self.collection_scope == Some(CollectionScope::Eden));
            self.update_allocation_limits();
        }
    }
}

pub struct DeferPoint {
    defers: &'static AtomicUsize,
}
impl DeferPoint {
    pub fn new(heap: &Heap) -> Self {
        let this = Self {
            defers: as_atomic!(& heap.defers;AtomicUsize),
        };
        this.defers.fetch_add(1, atomic::Ordering::SeqCst);
        this
    }
}

impl Drop for DeferPoint {
    fn drop(&mut self) {
        self.defers.fetch_sub(1, atomic::Ordering::SeqCst);
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        self.global.release_memory();
    }
}
#[inline(always)]
fn approximate_stack_pointer() -> *mut u8 {
    let mut result = null_mut();
    result = &mut result as *mut *mut u8 as *mut u8;
    result
}
