use std::{
    intrinsics::{likely, unlikely},
    mem::size_of,
    ptr::NonNull,
};

use crate::{
    api::{
        vtable_of, Collectable, Gc, HeapObjectHeader, ShadowStack, Trace, Visitor, MIN_ALLOCATION,
    },
    base::GcBase,
    bump_pointer_space::{align_usize, BumpPointerSpace},
    large_space::{LargeObjectSpace, PreciseAllocation},
};

pub const VERBOSE: bool = cfg!(feature = "minimark-verbose");
/// Generational garbage collector. It handles the objects in 2 generations:
///
/// - young objects: allocated in the nursery if they are not too large, or in LOS otherwise.
/// The nursery is fixed-size memory buffer of 4MB by default (or 1/2 of your L3 cache). When full,
/// we do a minor collection; the surviving objects from the nursery are moved outside, and the
/// non-surviving LOS objects are freed. All surviving objects become old.
///
/// - old objects: never move again. These objects are either allocated by mimalloc (if they are small),
/// or in LOS (if they are not small). Collected by regular mark-n-sweep during major collections.
///
/// ## Large objects
///
/// Large objects are allocated in [LargeObjectSpace](crate::large_space::LargeObjectSpace) and generational GC
/// works with them too. If large object is in young space then it is not marked in minor cycle. To promote large object
/// in minor GC cycle we just set its mark bit to 1. At start of each major collection mark bits of
/// large objects are cleared and all unmarked large objects at the end of the cycle are dead.
///
///
/// ## TODO
///
/// 1) old space might be compacted. To do so we have to implement our own allocation scheme. Some ideas:
/// - Use segregated free lists for allocating in old space
/// - When fragmentation is above some threshold (e.g 75%) we do compacting major collection
///
/// 2) We might make this GC semi-conservative like Mono's SGen GC. To do so we have to implement pinning for objects. Main problem
/// is pinning objects in nursery. Because they are pinned we have to track old objects referencing young pinned objects and that is not a simple
/// task. Also to identify objects on stack we might need bitmap to see if object pointer is indeed allocated in young or old space. Keeping that
/// bitmap is also additional memory and performance cost.
///
pub struct MiniMarkGC {
    nursery: BumpPointerSpace,
    old_space: OldSpace,
    los: LargeObjectSpace,
    mark_stack: Vec<*mut HeapObjectHeader>,
    stack: ShadowStack,
    young_objects_with_finalizers: Vector<*mut HeapObjectHeader>,
    objects_with_finalizers: Vector<*mut HeapObjectHeader>,
    remembered: Vec<*mut HeapObjectHeader>,
    major_collection_threshold: f64,
    next_major_collection_threshold: usize,
    next_major_collection_initial: usize,
    min_heap_size: usize,
    growth_rate_max: f64,
}

impl MiniMarkGC {
    pub fn old_space_allocated(&self) -> usize {
        self.old_space.allocated_bytes
    }

    pub fn is_young<T: Collectable + ?Sized>(&self, x: Gc<T>) -> bool {
        !self.is_old(x.base.as_ptr())
    }

    pub fn write_barrier<T: Collectable + ?Sized, U: Collectable + ?Sized>(
        &mut self,
        object: Gc<T>,
        field: Gc<U>,
    ) {
        unsafe {
            let base = object.base.as_ptr();
            let fbase = field.base.as_ptr();
            if self.is_old(base) && !self.is_old(fbase) {
                if !(*base).marked_bit() {
                    self.write_barrier_slow(base);
                }
            }
        }
    }
    #[cold]
    fn write_barrier_slow(&mut self, base: *mut HeapObjectHeader) {
        unsafe {
            (*base).set_marked_bit();
        }
        self.remembered.push(base);
    }

    fn is_old(&self, obj: *const HeapObjectHeader) -> bool {
        unsafe {
            if (*obj).is_precise() {
                //assert!(false);
                return (*PreciseAllocation::from_cell(obj as _)).is_marked();
            }
        }
        if self.nursery.contains(obj.cast()) {
            return false;
        }
        true
    }

    pub fn new(
        nursery_size: Option<usize>,
        min_heap_size: Option<usize>,
        growth_rate_max: Option<f64>,
    ) -> Box<Self> {
        let newsize = nursery_size.unwrap_or_else(|| 4 * 1024 * 1024);
        let mut this = Self {
            nursery: BumpPointerSpace::create("nursery", newsize),
            old_space: OldSpace {
                heap: unsafe { libmimalloc_sys::mi_heap_new() },
                allocated_bytes: 0,
            },
            young_objects_with_finalizers: Vector::new(),
            objects_with_finalizers: Vector::new(),
            mark_stack: vec![],
            los: LargeObjectSpace::new(),
            remembered: vec![],
            major_collection_threshold: 1.82,
            next_major_collection_initial: 0,
            next_major_collection_threshold: 0,
            min_heap_size: min_heap_size.unwrap_or_else(|| 8 * newsize),
            stack: ShadowStack::new(),
            growth_rate_max: growth_rate_max.unwrap_or_else(|| 1.4),
        };
        this.min_heap_size = this
            .min_heap_size
            .max((newsize as f64 * this.major_collection_threshold) as usize);

        this.next_major_collection_initial = this.min_heap_size;
        this.next_major_collection_threshold = this.min_heap_size;
        this.set_major_threshold_from(0.0);
        //   println!("{:p}->{:p}", this.nursery.begin(), this.nursery.limit());
        Box::new(this)
    }

    fn set_major_threshold_from(&mut self, mut threshold: f64) {
        let threshold_max =
            (self.next_major_collection_initial as f64 * self.growth_rate_max) as usize;

        if threshold > threshold_max as f64 {
            threshold = threshold_max as _;
        }
        if threshold < self.min_heap_size as f64 {
            threshold = self.min_heap_size as _;
        }
        self.next_major_collection_initial = threshold as _;
        self.next_major_collection_threshold = threshold as _;
    }

    fn deal_with_young_objects_with_finalizers(&mut self) {
        while let Some(object) = self.young_objects_with_finalizers.pop_back() {
            unsafe {
                if (*object).is_forwarded() {
                    let object = (*object).vtable() as *mut HeapObjectHeader;
                    self.objects_with_finalizers.push_back(object);
                } else if (*object).is_precise()
                    && (*PreciseAllocation::from_cell(object)).is_marked()
                {
                    self.objects_with_finalizers.push_back(object);
                } else {
                    (*object).get_dyn().finalize();
                }
            }
        }
    }

    fn deal_with_old_objects_with_finalizers(&mut self) {
        let mut new_objects = Vector::new();
        while let Some(object) = self.objects_with_finalizers.pop_back() {
            unsafe {
                if (*object).marked_bit() {
                    new_objects.push_back(object);
                } else {
                    (*object).get_dyn().finalize();
                }
            }
        }
        self.objects_with_finalizers = new_objects;
    }

    fn total_memory_used(&self) -> usize {
        self.old_space.allocated_bytes + self.los.bytes
    }
    /// To call when nursery is full. Do a minor collection, and possibly also a major collection,
    /// and finally reserve `totalsize` bytes at the start of the now-empty nursery.
    #[cold]
    #[inline(never)]
    fn collect_and_reserve(
        &mut self,
        totalsize: usize,
        keep: &mut [&mut dyn Trace],
    ) -> *mut HeapObjectHeader {
        self.minor_collection_(keep);
        if self.total_memory_used() > self.next_major_collection_threshold {
            self.major_collection_(keep);
        }
        unsafe {
            let mut result = self.nursery.alloc_thread_unsafe(totalsize, &mut 0, &mut 0);
            if result.is_null() {
                // The nursery might not be empty now, because of
                // finalizers.  If it is almost full again,
                // we need to fix it with another call to minor_collection().
                self.minor_collection_(keep);
                result = self.nursery.alloc_thread_unsafe(totalsize, &mut 0, &mut 0);
            }
            result
        }
    }

    /// Promotes object from nursery to old space.
    ///
    /// - nursery objects are malloc'ed in old space and copied to old space.
    /// - large objects are just marked until first major GC
    /// - old objects are skipped
    fn trace_drag_out(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        let obj = root.as_ptr();

        unsafe {
            if !self.nursery.contains(obj as _) {
                // if object is not in nursery, nothing to change -- except that
                // we must mark precise allocation.
                if (*obj).is_precise() && self.los.is_young(obj) {
                    (*PreciseAllocation::from_cell(obj)).test_and_set_marked();
                    if VERBOSE {
                        eprintln!(
                            "MiniMark: minor: promote preice allocation at {:p} to old",
                            obj
                        );
                    }
                    self.mark_stack.push(obj);
                }
                return;
            }

            if (*obj).is_forwarded() {
                *root = NonNull::new_unchecked((*obj).vtable() as _);
                return;
            }

            let newobj = self.malloc_out_of_nursery(obj);

            core::ptr::copy_nonoverlapping(obj.cast::<u8>(), newobj.cast::<u8>(), (*obj).size());
            (*newobj).unmark();
            (*obj).set_forwarded(newobj as _);
            *root = NonNull::new_unchecked(newobj);
            self.mark_stack.push(newobj);
            if VERBOSE {
                eprintln!(
                    "MiniMark: minor: promote young allocation at {:p} to {:p}",
                    obj, newobj
                );
            }
        }
    }

    fn minor_collection_(&mut self, keep: &mut [&mut dyn Trace]) {
        if VERBOSE {
            eprintln!("MiniMark: Minor collection");
        }
        self.los.prepare_for_marking(true);
        self.los.begin_marking(false);

        for ref_ in keep {
            ref_.trace(&mut YoungTrace { gc: self });
        }

        let stack: &'static ShadowStack = unsafe { std::mem::transmute(&self.stack) };

        unsafe {
            stack.walk(|entry| {
                entry.trace(&mut YoungTrace { gc: self });
            });

            while let Some(object) = self.remembered.pop() {
                (*object).unmark();

                (*object).get_dyn().trace(&mut YoungTrace { gc: self });
            }

            while let Some(object) = self.mark_stack.pop() {
                (*object).get_dyn().trace(&mut YoungTrace { gc: self });
            }
        }
        if !self.young_objects_with_finalizers.is_empty() {
            self.deal_with_young_objects_with_finalizers();
        }

        let begin = self.nursery.begin();
        self.nursery.set_end(begin);
        self.los.prepare_for_allocation(true);
        self.los.sweep();
    }
    fn major_collection_(&mut self, keep: &mut [&mut dyn Trace]) {
        if VERBOSE {
            eprintln!("MiniMark: Major collection");
        }
        self.los.prepare_for_marking(false);
        self.los.begin_marking(true);

        unsafe {
            while let Some(object) = self.remembered.pop() {
                (*object).unmark();
            }
        }

        for ref_ in keep {
            ref_.trace(&mut OldTrace { gc: self });
        }

        let stack: &'static ShadowStack = unsafe { std::mem::transmute(&self.stack) };

        unsafe {
            stack.walk(|entry| {
                entry.trace(&mut OldTrace { gc: self });
            });

            while let Some(object) = self.mark_stack.pop() {
                (*object).get_dyn().trace(&mut OldTrace { gc: self });
            }
        }
        if !self.objects_with_finalizers.is_empty() {
            self.deal_with_old_objects_with_finalizers();
        }

        self.old_space.sweep();
        let total_memory_used = self.total_memory_used();
        self.set_major_threshold_from(total_memory_used as f64 * self.major_collection_threshold);
        self.los.prepare_for_allocation(false);
        self.los.sweep();
    }

    fn malloc_out_of_nursery(&mut self, object: *mut HeapObjectHeader) -> *mut HeapObjectHeader {
        unsafe {
            let size = (*object).size();
            return self.old_space.alloc(size).cast();
        }
    }

    fn is_marked_old(&self, object: *mut HeapObjectHeader) -> bool {
        unsafe {
            if (*object).is_precise() {
                return (*PreciseAllocation::from_cell(object)).is_marked();
            } else {
                (*object).marked_bit()
            }
        }
    }

    fn mark_old(&self, object: *mut HeapObjectHeader) {
        unsafe {
            if (*object).is_precise() {
                (*PreciseAllocation::from_cell(object)).mark = true;
            }
            (*object).set_marked_bit();
        }
    }
}
struct OldTrace<'a> {
    gc: &'a mut MiniMarkGC,
}

impl<'a> Visitor for OldTrace<'a> {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        if !self.gc.is_marked_old(root.as_ptr()) {
            self.gc.mark_old(root.as_ptr());
            self.gc.mark_stack.push(root.as_ptr());
        }
    }
}
struct YoungTrace<'a> {
    gc: &'a mut MiniMarkGC,
}

impl<'a> Visitor for YoungTrace<'a> {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        self.gc.trace_drag_out(root);
    }
}

use im::Vector;
use libmimalloc_sys::{mi_heap_area_t, mi_heap_t};

struct OldSpace {
    heap: *mut mi_heap_t,
    allocated_bytes: usize,
}

impl OldSpace {
    #[inline]
    fn alloc(&mut self, size: usize) -> *mut u8 {
        self.allocated_bytes += size;
        let ptr = unsafe {
            libmimalloc_sys::mi_heap_malloc_aligned(self.heap, size, MIN_ALLOCATION).cast()
        };

        ptr
    }

    fn sweep(&mut self) {
        unsafe {
            self.allocated_bytes = 0;
            unsafe extern "C" fn visitor(
                _heap: *const mi_heap_t,
                _area: *const mi_heap_area_t,
                block: *mut libc::c_void,
                block_size: usize,
                arg: *mut libc::c_void,
            ) -> bool {
                let old_space = &mut *arg.cast::<OldSpace>();
                if block.is_null() {
                    return true;
                }

                let object = block.cast::<HeapObjectHeader>();
                if (*object).marked_bit() {
                    (*object).unmark();
                    old_space.allocated_bytes += block_size;
                } else {
                    if VERBOSE {
                        eprintln!("Free old object {:p} {} bytes", block, block_size);
                    }
                    libmimalloc_sys::mi_free(block);
                }

                true
            }
            libmimalloc_sys::mi_heap_visit_blocks(
                self.heap,
                true,
                Some(visitor),
                self as *mut Self as _,
            );
        }
    }
}

impl GcBase for MiniMarkGC {
    /*fn add_local_scope(&mut self, scope: &mut LocalScope) {
        if self.head.is_null() && self.tail.is_null() {
            self.head = scope as *mut _;
            self.tail = scope as *mut _;
            scope.next = null_mut();
            scope.prev = null_mut();
        } else {
            scope.prev = self.tail;
            scope.next = null_mut();
            unsafe {
                (*self.tail).next = scope as *mut _;
                self.tail = scope as *mut _;
            }
        }
    }*/
    fn finalize_handlers(&self) -> &Vector<*mut HeapObjectHeader> {
        &self.objects_with_finalizers
    }
    fn finalize_handlers_mut(&mut self) -> &mut Vector<*mut HeapObjectHeader> {
        &mut self.objects_with_finalizers
    }
    #[inline(always)]
    fn shadow_stack<'a>(&self) -> &'a ShadowStack {
        unsafe { std::mem::transmute(&self.stack) }
    }

    /// Performs minor collection cycle and if old space is full performs major collection.
    fn collect(&mut self, refs: &mut [&mut dyn Trace]) {
        self.minor_collection_(refs);
        if self.total_memory_used() > self.next_major_collection_threshold {
            self.major_collection_(refs);
        }
    }
    fn set_finalize_lock(&mut self, _x: bool) {}
    fn finalize_lock(&self) -> bool {
        false
    }
    #[inline(always)]
    fn allocate<T: Collectable + 'static>(&mut self, mut value: T) -> Gc<T> {
        let size = align_usize(
            value.allocation_size() + size_of::<HeapObjectHeader>(),
            MIN_ALLOCATION,
        );
        unsafe {
            let mut memory = if likely(size < 64 * 1024) {
                self.nursery.alloc_thread_unsafe(size, &mut 0, &mut 0)
            } else {
                self.los.allocate(size)
            };
            if unlikely(memory.is_null()) {
                memory = self.collect_and_reserve(size, &mut [&mut value]);
            }
            // self.total_allocations += size;
            memory.write(HeapObjectHeader {
                value: 0,
                type_id: crate::small_type_id::<T>(),
                padding: 0,
            });
            (*memory).set_vtable(vtable_of::<T>());
            if size <= 64 * 1024 {
                (*memory).set_size(size);
            } else {
                (*memory).set_size(0);
            }
            ((*memory).data() as *mut T).write(value);
            if std::mem::needs_drop::<T>() {
                self.young_objects_with_finalizers.push_back(memory);
            }
            // self.num_allocated_since_last_gc += 1;
            Gc {
                base: NonNull::new_unchecked(memory),
                marker: Default::default(),
            }
        }
    }
    #[inline]
    fn allocate_safe<T: Collectable + 'static>(
        &mut self,
        value: T,
        refs: &mut [&mut dyn Trace],
    ) -> Gc<T> {
        let stack = self.shadow_stack();
        letroot!(refs = stack, refs);

        let result = self.allocate(value);
        drop(refs);
        result
    }
    #[inline(always)]
    fn try_allocate<T: Collectable + 'static>(&mut self, value: T) -> Result<crate::api::Gc<T>, T> {
        let size = align_usize(
            value.allocation_size() + size_of::<HeapObjectHeader>(),
            MIN_ALLOCATION,
        );
        unsafe {
            let memory = if likely(size < 64 * 1024) {
                self.nursery.alloc_thread_unsafe(size, &mut 0, &mut 0)
            } else {
                self.los.allocate(size)
            };
            if unlikely(memory.is_null()) {
                return Err(value);
            }
            // self.total_allocations += size;
            memory.write(HeapObjectHeader {
                value: 0,
                type_id: crate::small_type_id::<T>(),
                padding: 0,
            });
            (*memory).set_vtable(vtable_of::<T>());
            if size <= 64 * 1024 {
                (*memory).set_size(size);
            } else {
                (*memory).set_size(0);
            }
            ((*memory).data() as *mut T).write(value);
            if std::mem::needs_drop::<T>() {
                self.young_objects_with_finalizers.push_back(memory);
            }
            // self.num_allocated_since_last_gc += 1;
            Ok(Gc {
                base: NonNull::new_unchecked(memory),
                marker: Default::default(),
            })
        }
    }

    /// Performs minor GC cycle. It just copies all surviving objects from nursery to old space.
    fn minor_collection(&mut self, refs: &mut [&mut dyn Trace]) {
        self.minor_collection_(refs);
    }

    /// Performs full GC cycle. It includes both minor and major cycles.
    #[inline(never)]
    fn full_collection(&mut self, refs: &mut [&mut dyn Trace]) {
        self.minor_collection_(refs);
        self.major_collection_(refs);
    }

    fn register_finalizer<T: Collectable + ?Sized>(&mut self, object: Gc<T>) {
        self.young_objects_with_finalizers
            .push_back(object.base.as_ptr());
    }

    fn allocate_raw<T: Collectable>(
        &mut self,
        size: usize,
    ) -> Option<Gc<std::mem::MaybeUninit<T>>> {
        let size = align_usize(size + size_of::<HeapObjectHeader>(), MIN_ALLOCATION);
        unsafe {
            let memory = if likely(size < 64 * 1024) {
                self.nursery.alloc_thread_unsafe(size, &mut 0, &mut 0)
            } else {
                self.los.allocate(size)
            };
            if unlikely(memory.is_null()) {
                return None;
            }

            // self.total_allocations += size;
            memory.write(HeapObjectHeader {
                value: 0,
                type_id: crate::small_type_id::<T>(),
                padding: 0,
            });
            (*memory).set_vtable(vtable_of::<T>());
            if size <= 64 * 1024 {
                (*memory).set_size(size);
            } else {
                (*memory).set_size(0);
            }

            Some(Gc {
                base: NonNull::new_unchecked(memory),
                marker: Default::default(),
            })
        }
    }
}
