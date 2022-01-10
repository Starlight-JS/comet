//! # MiniMark
//! Generational garbage collector. It handles the objects in 2 generations:
//!
//! - young objects: allocated in the nursery if they are not too large, or in LOS otherwise.
//! The nursery is fixed-size memory buffer of 4MB by default (or 1/2 of your L3 cache). When full,
//! we do a minor collection; the surviving objects from the nursery are moved outside, and the
//! non-surviving LOS objects are freed. All surviving objects become old.
//!
//! - old objects: never move again. These objects are either allocated by [rosalloc](https://github.com/playxe/rosalloc) (if they are small),
//! or in LOS (if they are not small). Collected by regular mark-n-sweep during major collections.
//!
//! ## Large objects
//!
//! Large objects are allocated in [LargeObjectSpace](crate::large_space::LargeObjectSpace) and generational GC
//! works with them too. If large object is in young space then it is not marked in minor cycle. To promote large object
//! in minor GC cycle we just set its mark bit to 1. At start of each major collection mark bits of
//! large objects are cleared and all unmarked large objects at the end of the cycle are dead.
use crate::api::vtable_of;
use crate::api::Collectable;
use crate::api::Gc;
use crate::api::Weak;
use crate::api::GC_BLACK;
use crate::api::GC_GREY;
use crate::api::GC_WHITE;
use crate::gc_base::AllocationSpace;
use crate::gc_base::GcBase;
use crate::gc_base::MarkingConstraint;
use crate::gc_base::MarkingConstraintRuns;
use crate::gc_base::NoReadBarrier;
use crate::gc_base::TLAB;
use crate::large_space::LargeObjectSpace;
use crate::mutator::*;
use crate::rosalloc_space::TLABWithRuns;
use crate::safepoint::*;
use crate::small_type_id;
use crate::utils::align_usize;
use crate::{
    api::{HeapObjectHeader, Trace, Visitor},
    bump_pointer_space::BumpPointerSpace,
    large_space::PreciseAllocation,
    rosalloc_space::RosAllocSpace,
    utils::formatted_size,
};
use atomic::Atomic;
//use atomic::Atomic;
use atomic::Ordering;
use im::Vector;
use parking_lot::{lock_api::RawMutex, RawMutex as Lock};
use rosalloc::dedicated_full_run;
use rosalloc::Rosalloc;
use rosalloc::Run;
use rosalloc::NUM_OF_SLOTS;
//use rosalloc::defs::NUM_THREAD_LOCAL_SIZE_BRACKETS;
use rosalloc::defs::NUM_THREAD_LOCAL_SIZE_BRACKETS;
//use rosalloc::Run;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::mem::size_of;
use std::ptr::{null_mut, NonNull};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GcReason {
    RequestedByUser,
    AllocationFailure,
    OldSpaceFull,
}

/// Generational garbage collector. It handles the objects in 2 generations:
///
/// - young objects: allocated in the nursery if they are not too large, or in LOS otherwise.
/// The nursery is fixed-size memory buffer of 4MB by default (or 1/2 of your L3 cache). When full,
/// we do a minor collection; the surviving objects from the nursery are moved outside, and the
/// non-surviving LOS objects are freed. All surviving objects become old.
///
/// - old objects: never move again. These objects are either allocated by [rosalloc](https://github.com/playxe/rosalloc) (if they are small),
/// or in LOS (if they are not small). Collected by regular mark-n-sweep during major collections.
///
/// ## Large objects
///
/// Large objects are allocated in [LargeObjectSpace](crate::large_space::LargeObjectSpace) and generational GC
/// works with them too. If large object is in young space then it is not marked in minor cycle. To promote large object
/// in minor GC cycle we just set its mark bit to 1. At start of each major collection mark bits of
/// large objects are cleared and all unmarked large objects at the end of the cycle are dead.
pub struct MiniMark {
    nursery: BumpPointerSpace,
    pub(crate) global_heap_lock: Lock,
    pub(crate) large_space_lock: Lock,
    mutators: Vec<*mut Mutator<Self>>,
    safepoint: GlobalSafepoint,
    large_space: LargeObjectSpace,
    old_space: *mut RosAllocSpace,

    verbose: bool,

    mark_stack: Vec<*mut HeapObjectHeader>,
    old_mark_stack: Vec<*mut HeapObjectHeader>,
    num_old_space_allocated: Atomic<usize>,

    total_gcs: usize,
    remembered_set: Vec<*mut HeapObjectHeader>,
    rem_set_lock: Lock,

    major_collection_threshold: f64,
    next_major_collection_threshold: Atomic<usize>,
    next_major_collection_initial: Atomic<usize>,
    min_heap_size: usize,
    growth_rate_max: f64,
    promoted: usize,

    gc_state: Atomic<MajorPhase>,
    alloc_color: u8,
    mark_color: u8,
    growth_limit: usize,
    weak_refs: Vec<Weak<dyn Collectable, Self>>,
    constraints: Vec<Box<dyn MarkingConstraint>>,
    finalize_list: Vector<*mut HeapObjectHeader>,
    finalize_lock: Lock,
    finalize_list_old: Vector<*mut HeapObjectHeader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MajorPhase {
    Scanning,
    Marking,
    Sweeping,
    Finalizing,
}

pub struct MiniMarkOptions {
    pub verbose: bool,
    pub nursery_size: usize,
    pub initial_size: usize,
    pub growth_limit: usize,
    pub min_heap_size: usize,
    pub capacity: usize,
    pub low_memory_mode: bool,
    pub growth_rate_max: f64,
}

impl Default for MiniMarkOptions {
    fn default() -> Self {
        Self {
            verbose: false,
            nursery_size: 32 * 1024 * 1024,
            initial_size: 128 * 1024 * 1024,
            growth_limit: 512 * 1024 * 1024,
            min_heap_size: 64 * 1024 * 1024,
            capacity: 512 * 1024 * 1024,
            low_memory_mode: false,
            growth_rate_max: 1.4,
        }
    }
}

pub fn instantiate_minimark(options: MiniMarkOptions) -> MutatorRef<MiniMark> {
    let heap = Arc::new(UnsafeCell::new(MiniMark::new(
        options.verbose,
        Some(options.nursery_size),
        options.initial_size,
        options.growth_limit,
        options.min_heap_size,
        options.capacity,
        options.low_memory_mode,
        Some(options.growth_rate_max),
    )));
    let href = unsafe { &mut *heap.get() };
    let join_data = JoinData::new();
    let mut mutator = MutatorRef::new(Mutator::new(
        heap.clone(),
        &href.safepoint,
        join_data.internal.clone(),
    ));
    href.mutators.push(&mut *mutator);
    href.safepoint
        .n_mutators
        .fetch_add(1, atomic::Ordering::Relaxed);
    mutator.state_set(ThreadState::Safe, ThreadState::Unsafe);
    mutator
}

impl MiniMark {
    fn new(
        verbose: bool,
        nursery_size: Option<usize>,
        initial_size: usize,
        growth_limit: usize,
        min_heap_size: usize,
        capacity: usize,
        low_memory_mode: bool,
        growth_rate_max: Option<f64>,
    ) -> Self {
        let growth_limit = capacity.min(growth_limit);
        let rosalloc = RosAllocSpace::create(
            "old-space",
            initial_size,
            growth_limit,
            capacity,
            low_memory_mode,
            false,
        );

        let mut this = Self {
            finalize_list: Vector::new(),
            finalize_list_old: Vector::new(),
            finalize_lock: Lock::INIT,
            growth_limit,
            constraints: vec![],
            alloc_color: GC_WHITE,
            old_mark_stack: Vec::new(),
            mark_color: GC_BLACK,
            gc_state: Atomic::new(MajorPhase::Scanning),
            global_heap_lock: Lock::INIT,
            large_space_lock: Lock::INIT,
            large_space: LargeObjectSpace::new(),
            mutators: vec![],
            promoted: 0,
            safepoint: GlobalSafepoint::new(),
            mark_stack: vec![],
            remembered_set: vec![],
            rem_set_lock: Lock::INIT,
            nursery: BumpPointerSpace::new(nursery_size.unwrap_or_else(|| 32 * 1024 * 1024)),
            verbose,
            total_gcs: 0,
            min_heap_size,
            growth_rate_max: growth_rate_max.unwrap_or_else(|| 1.4),
            major_collection_threshold: 1.82,
            next_major_collection_initial: Atomic::new(0),
            next_major_collection_threshold: Atomic::new(0),
            num_old_space_allocated: Atomic::new(0),
            old_space: rosalloc,
            weak_refs: vec![],
        };
        this.min_heap_size = this
            .min_heap_size
            .max((this.nursery.size() as f64 * this.major_collection_threshold) as usize);

        this.next_major_collection_initial
            .store(this.min_heap_size, Ordering::Relaxed);
        this.next_major_collection_threshold
            .store(this.min_heap_size, Ordering::Release);
        this.set_major_threshold_from(0.0);
        this
    }
    #[allow(dead_code)]
    fn wait_for_gc_to_complete_locked(&mut self) -> bool {
        /* let did_gc = if let Some(task) = self.gc_task.take() {
            let (time, sweeped) = task.join();
            let prev = self.num_old_space_allocated;
            self.num_old_space_allocated -= sweeped;
            if self.verbose {
                eprintln!(
                    "[gc] Concurrent GC end: {}->{} {:.4}ms",
                    formatted_size(prev),
                    formatted_size(self.num_old_space_allocated),
                    time
                );
            }
            let total_bytes = self.num_old_space_allocated + self.large_space.bytes;
            self.set_major_threshold_from(total_bytes as f64 * self.major_collection_threshold);
            true
        } else {
            false
        };

        did_gc*/
        return false;
    }
    fn wait_for_gc_to_complete(&mut self) -> bool {
        /*self.large_space_lock.lock();
        self.gc_task_lock.lock();
        let did_gc = self.wait_for_gc_to_complete_locked();
        unsafe {
            self.large_space_lock.unlock();
            self.gc_task_lock.unlock();
        }
        did_gc*/
        return false;
    }

    unsafe fn trace_drag_out(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        root: &mut NonNull<HeapObjectHeader>,
        _parent: *mut HeapObjectHeader,
    ) {
        // todo: use `parent` for object pinning support
        let object = root.as_ptr();

        if self.nursery.contains(object.cast()) {
            if (*object).is_forwarded() {
                *root = NonNull::new_unchecked((*object).vtable() as _);
            } else {
                let size = (*object).size();
                let mut tl_bulk_allocated = 0;

                let memory = (*self.old_space).alloc_common::<Self, true>(
                    mutator,
                    size,
                    &mut 0,
                    &mut 0,
                    &mut tl_bulk_allocated,
                );

                // No memory left in old space: just abort current process
                if memory.is_null() {
                    promotion_oom(size, object.cast());
                }

                self.promoted += size;
                {
                    // copy young object to memory in old space and update forwarding pointer
                    std::ptr::copy_nonoverlapping(object.cast::<u8>(), memory.cast::<u8>(), size);
                    (*object).set_forwarded(memory as _);
                    *root = NonNull::new_unchecked(memory.cast());
                    (*memory.cast::<HeapObjectHeader>()).force_set_color(self.mark_color);
                }
                (*(*self.old_space).get_live_bitmap()).set(memory);

                // incremase num_old_space_allocated. If at the end of minor collection it is larger than target footprint we perform major collection
                self.num_old_space_allocated
                    .fetch_add(tl_bulk_allocated, Ordering::AcqRel);
                self.mark_stack.push(memory.cast());
            }
        } else if (*object).is_precise() {
            // To promote LOS object we just set it as marked. During sweeping LOS at minor collection we do not clear these marks
            // but before performing major collection we do clear them so they can be finally be sweeped
            if !(*PreciseAllocation::from_cell(object)).test_and_set_marked() {
                self.promoted += (*PreciseAllocation::from_cell(object)).cell_size();
                self.mark_stack.push(object);
            }
        } else {
            // If we end up here then we're probably processing remembered set
            // we have to do nothing with old space object.
            debug_assert!((*self.old_space).has_address(object.cast()));
        }
    }

    unsafe fn trace(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        let object = root.as_ptr();

        if (*object).is_precise() {
            if !(*PreciseAllocation::from_cell(object)).test_and_set_marked() {
                (*object).force_set_color(GC_GREY);
                self.old_mark_stack.push(object);
            }
        } else if (*self.old_space).has_address(object.cast()) {
            if !(*object).set_color(self.alloc_color, GC_GREY) {
                self.old_mark_stack.push(object);
            }
        } else {
        }
    }
    pub fn is_young(&self, ptr: *mut HeapObjectHeader) -> bool {
        unsafe {
            self.nursery.contains(ptr.cast())
                || (*ptr).is_precise() && !(*PreciseAllocation::from_cell(ptr)).is_marked()
        }
    }
    unsafe fn after_mark_constraints(&mut self, mutator: &mut MutatorRef<Self>, young: bool) {
        let this = self as *mut Self;
        (*this).constraints.retain_mut(|constraint| {
            if constraint.is_over() {
                false
            } else {
                if constraint.runs_at() == MarkingConstraintRuns::AfterMark {
                    if young {
                        constraint.run(&mut YoungVisitor {
                            parent_object: null_mut(),
                            minimark: self,
                            mutator,
                        });
                    } else {
                        constraint.run(&mut OldVisitor { minimark: self });
                    }
                }
                true
            }
        });
    }
    unsafe fn before_mark_constraints(&mut self, mutator: &mut MutatorRef<Self>, young: bool) {
        let this = self as *mut Self;
        (*this).constraints.retain_mut(|constraint| {
            if constraint.is_over() {
                false
            } else {
                if constraint.runs_at() == MarkingConstraintRuns::BeforeMark {
                    if young {
                        constraint.run(&mut YoungVisitor {
                            parent_object: null_mut(),
                            minimark: self,
                            mutator,
                        });
                    } else {
                        constraint.run(&mut OldVisitor { minimark: self });
                    }
                }
                true
            }
        });
    }
    unsafe fn revoke_tlabs_young(&mut self, self_thread: &mut MutatorRef<Self>) {
        let mut revoke_freed = 0;
        for i in 0..self.mutators.len() {
            let mutator = self.mutators[i];
            revoke_freed +=
                (*(*self.old_space).rosalloc()).revoke_thread_local_runs(&mut (*mutator).tlab.runs);
            (*mutator).reset_tlab();
            (*mutator).shadow_stack().walk(|var| {
                var.trace(&mut YoungVisitor {
                    minimark: self,
                    parent_object: null_mut(),
                    mutator: self_thread,
                });
            });
        }
        (*(*self.old_space).rosalloc()).revoke_thread_unsafe_current_runs();
        self.num_old_space_allocated
            .fetch_sub(revoke_freed, Ordering::AcqRel);
    }

    unsafe fn major_marking_phase(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        keep: &mut [&mut dyn Trace],
    ) {
        self.before_mark_constraints(mutator, false);
        keep.iter_mut().for_each(|item| {
            item.trace(&mut OldVisitor { minimark: self });
        });
        let mut revoke_freed = 0;
        // some of mutators might allocate into TLS runs while performing minor collection
        // so we iterate all mutators and revoke all runs from them. Note that most of the time
        // these runs point to `rosalloc::dedicated_full_run()` so revoking is basically no-op and
        // does not do any time consuming operations.
        for i in 0..self.mutators.len() {
            let mutator = self.mutators[i];

            revoke_freed +=
                (*(*self.old_space).rosalloc()).revoke_thread_local_runs(&mut (*mutator).tlab.runs);

            (*(*self.old_space).rosalloc()).revoke_thread_unsafe_current_runs();
            (*mutator).reset_tlab();
            (*mutator)
                .shadow_stack()
                .walk(|var| var.trace(&mut OldVisitor { minimark: self }));
        }

        // process remembered set.
        //
        //
        // Note that at the moment remset is always empty at major collection because our marking phase is not concurrent/incremental
        while let Some(object) = self.remembered_set.pop() {
            (*object)
                .get_dyn()
                .trace(&mut OldVisitor { minimark: self });
            (*object).unmark();
        }

        // Drain mark stack and process object references
        while let Some(object) = self.old_mark_stack.pop() {
            (*object)
                .get_dyn()
                .trace(&mut OldVisitor { minimark: self });
            (*object).force_set_color(self.mark_color);
        }
        self.after_mark_constraints(mutator, false);
        let color = self.mark_color;

        // Get rid of dead weak references
        self.weak_refs.retain_mut(|object| {
            let header = object.base();
            if (*header).get_color() == color {
                object.after_mark(|header| {
                    if (*header).get_color() == color {
                        header
                    } else {
                        null_mut()
                    }
                });
                true
            } else {
                false
            }
        });

        self.num_old_space_allocated
            .fetch_sub(revoke_freed, Ordering::Relaxed);
    }

    unsafe fn minor_marking_phase(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        keep: &mut [&mut dyn Trace],
    ) {
        self.large_space.prepare_for_marking(true);
        self.large_space.begin_marking(false);
        let self_thread = mutator;
        self.before_mark_constraints(self_thread, true);
        self.revoke_tlabs_young(self_thread);

        keep.iter_mut().for_each(|item| {
            item.trace(&mut YoungVisitor {
                minimark: self,
                parent_object: null_mut(),
                mutator: self_thread,
            });
        });

        while let Some(object) = self.remembered_set.pop() {
            println!("remembered");
            (*object).get_dyn().trace(&mut YoungVisitor {
                minimark: self,
                parent_object: object,
                mutator: self_thread,
            });
            (*object).unmark();
        }

        while let Some(object) = self.mark_stack.pop() {
            (*object).get_dyn().trace(&mut YoungVisitor {
                minimark: self,
                parent_object: object,
                mutator: self_thread,
            });
            (*object).set_color(GC_GREY, GC_BLACK);
        }
        self.after_mark_constraints(self_thread, true);
    }
    unsafe fn minor(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        keep: &mut [&mut dyn Trace],
        reason: GcReason,
    ) -> bool {
        // threads must be suspended already
        let time = if self.verbose {
            Some(std::time::Instant::now())
        } else {
            None
        };

        self.large_space.prepare_for_marking(true);
        self.large_space.begin_marking(false);
        self.minor_marking_phase(mutator, keep);
        let nursery_start = self.nursery.start();
        let nursery_end = self.nursery.end();

        let mut finalize_list_old = mem::replace(&mut self.finalize_list_old, Vector::new());
        self.finalize_list.retain(|x| {
            let object = *x;
            if (*object).is_forwarded() {
                finalize_list_old.push_back((*object).vtable() as _);
            } else if (*PreciseAllocation::from_cell(object)).is_marked() {
                finalize_list_old.push_back(object);
            }
            (*object).get_dyn().finalize();
            false
        });

        let is_young = |ptr: *mut HeapObjectHeader| {
            (ptr.cast::<u8>() >= nursery_start && ptr.cast::<u8>() < nursery_end)
                || (*ptr).is_precise() && !(*PreciseAllocation::from_cell(ptr)).is_marked()
        };
        self.weak_refs.retain_mut(|object| {
            let header = object.base();
            if is_young(header) {
                if (*header).is_forwarded() {
                    object.set_base((*header).vtable() as _);
                    object.after_mark(|header| {
                        if !(*header).is_forwarded() && is_young(header) {
                            null_mut()
                        } else if (*header).is_forwarded() {
                            (*header).vtable() as *mut HeapObjectHeader
                        } else {
                            // LOS or old space object
                            header
                        }
                    });
                    true
                } else {
                    false
                }
            } else {
                true
            }
        });
        self.large_space.prepare_for_allocation(true);
        self.large_space.sweep();

        self.nursery.reset();

        if let Some(time) = time {
            let elapsed = time.elapsed();
            eprintln!(
                "[gc] GC({}) Pause Young ({:?}) Promoted {}(old space: {} {}->{}) {:.4}ms",
                self.total_gcs,
                reason,
                formatted_size(self.promoted),
                formatted_size(
                    self.num_old_space_allocated.load(Ordering::Relaxed) + self.large_space.bytes
                ),
                print_color(self.alloc_color),
                print_color(self.mark_color),
                elapsed.as_micros() as f64 / 1000.0
            )
        }
        self.total_gcs += 1;

        self.promoted = 0;

        self.large_space.bytes + self.num_old_space_allocated.load(Ordering::Relaxed)
            > self.next_major_collection_threshold.load(Ordering::Acquire)
    }

    unsafe fn major(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        keep: &mut [&mut dyn Trace],
        reason: GcReason,
    ) {
        let time = if self.verbose {
            Some(std::time::Instant::now())
        } else {
            None
        };
        let prev = self.num_old_space_allocated.load(Ordering::Relaxed) + self.large_space.bytes;
        self.major_marking_phase(mutator, keep);

        let rosalloc = self.old_space as usize;
        let sweep_color = self.alloc_color;
        let keep_color = self.mark_color;
        let this = self as *mut Self as usize;

        self.large_space.prepare_for_allocation(false);
        self.large_space.sweep();
        self.gc_state.store(MajorPhase::Sweeping, Ordering::Relaxed);

        let rosalloc = rosalloc as *mut RosAllocSpace;
        let this = &mut *(this as *mut Self);

        let (freed, _) = (*rosalloc).sweep_colored(
            |pointers, _| (*rosalloc).sweep_callback(pointers, false),
            sweep_color,
            keep_color,
        );
        (*(*rosalloc).rosalloc()).trim();
        this.num_old_space_allocated
            .fetch_sub(freed, Ordering::Relaxed);
        let total_bytes =
            this.num_old_space_allocated.load(Ordering::Acquire) + this.large_space.bytes;
        this.set_major_threshold_from(total_bytes as f64 * this.major_collection_threshold);

        this.gc_state.store(MajorPhase::Scanning, Ordering::Relaxed);

        if let Some(time) = time {
            eprintln!(
                "[gc] GC({}) Pause Old ({:?}) {}->{}({}) {:.4}ms",
                self.total_gcs,
                reason,
                formatted_size(prev),
                formatted_size(
                    self.num_old_space_allocated.load(Ordering::Relaxed) + self.large_space.bytes
                ),
                formatted_size(self.next_major_collection_threshold.load(Ordering::Relaxed)),
                time.elapsed().as_micros() as f64 / 1000.0
            )
        }
        self.total_gcs += 1;
    }

    fn set_major_threshold_from(&mut self, mut threshold: f64) {
        let threshold_max = (self.next_major_collection_initial.load(Ordering::Relaxed) as f64
            * self.growth_rate_max) as usize;

        if threshold > threshold_max as f64 {
            threshold = threshold_max as _;
        }
        if threshold < self.min_heap_size as f64 {
            threshold = self.min_heap_size as _;
        }
        self.next_major_collection_initial
            .store(threshold as _, Ordering::Relaxed);
        self.next_major_collection_threshold
            .store(threshold as _, Ordering::Release);
        if self.verbose {
            eprintln!(
                "[gc] Major threshold set to {}",
                formatted_size(threshold as _)
            );
        }
    }
    #[inline(always)]
    unsafe fn write_barrier_internal(&mut self, object: *mut HeapObjectHeader) {
        if (*self.old_space).has_address(object.cast())
            || ((*object).is_precise() && (*PreciseAllocation::from_cell(object)).is_marked())
        // mark bit in LOS object header means it is in large old object
        {
            if !(*object).marked_bit() {
                self.write_barrier_slow(object);
            }
        }
    }

    #[cold]
    unsafe fn write_barrier_slow(&mut self, object: *mut HeapObjectHeader) {
        (*object).set_marked_bit(); // marked_bit is used for seeing if object is in remembered set
        self.rem_set_lock.lock();
        self.remembered_set.push(object);
        self.rem_set_lock.unlock();
    }

    #[inline]
    fn alloc_inline_old<T: crate::api::Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: T,
        _space: AllocationSpace,
    ) -> Gc<T, Self> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        let val = if Rosalloc::is_size_for_thread_local(size) {
            let (idx, _bracket_size) = Rosalloc::size_to_index_and_bracket_size(size);
            unsafe {
                let thread_local_run = &mut *mutator.tlab.runs[idx];

                let slot_addr = (*thread_local_run).alloc_slot();
                if slot_addr.is_null() {
                    return self.alloc_once::<T, false, true>(mutator, value);
                }

                let header = slot_addr.cast::<HeapObjectHeader>();
                header.write(HeapObjectHeader {
                    type_id: small_type_id::<T>(),
                    padding: 0,
                    padding2: 0,
                    value: 0,
                });
                (*header).set_vtable(vtable_of::<T>());
                (*header).set_size(size);
                ((*header).data() as *mut T).write(value);
                Gc {
                    base: NonNull::new_unchecked(header),
                    marker: PhantomData,
                }
            }
        } else {
            self.alloc_once::<T, false, true>(mutator, value)
        };
        self.post_alloc(val);
        val
    }

    #[inline]
    fn alloc_inline_new<T: crate::api::Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        mut value: T,
        _space: AllocationSpace,
    ) -> Gc<T, Self> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        let mut memory = self.nursery.bump_alloc(size);
        if memory.is_null() {
            self.collect_alloc_failure(mutator, &mut [&mut value]);

            //self.collect(mutator, &mut [&mut value]);
            memory = self.nursery.bump_alloc(size);
            if memory.is_null() {
                oom_abort();
            }
        }

        unsafe {
            let hdr = memory.cast::<HeapObjectHeader>();
            (*hdr).set_vtable(vtable_of::<T>());
            (*hdr).set_size(size);

            ((*hdr).data() as *mut T).write(value);
            let val = Gc {
                base: NonNull::new_unchecked(hdr),
                marker: Default::default(),
            };
            self.post_alloc(val);
            val
        }
    }
    #[cold]
    pub fn alloc_slow<T: Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        mut value: T,
    ) -> Gc<T, Self> {
        self.full_collection(mutator, &mut [&mut value]);
        self.alloc_once::<T, true, false>(mutator, value)
    }

    #[cold]
    #[inline(never)]
    pub fn alloc_once<T: Collectable + Sized + 'static, const GROW: bool, const GC: bool>(
        &mut self,
        mut mutator: &mut MutatorRef<Self>,
        value: T,
    ) -> Gc<T, Self> {
        fn max_bytes_bulk_allocated_for(size: usize) -> usize {
            if !Rosalloc::is_size_for_thread_local(size) {
                return size;
            }
            let (idx, bracket_size) = Rosalloc::size_to_index_and_bracket_size(size);
            NUM_OF_SLOTS[idx] * bracket_size
        }
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        let max_bytes_tl_bulk_allocated = max_bytes_bulk_allocated_for(size);
        if self.is_out_of_memory_on_allocation(max_bytes_tl_bulk_allocated, GROW) {
            // potentially run GC if we reached GC threshold

            return self.alloc_slow(mutator, value);
        }
        let mut bytes_allocated = 0;
        let mut usable_size = 0;
        let mut bytes_tl_bulk_allocated = 0;
        unsafe {
            let mem = (*self.old_space).alloc_common::<Self, true>(
                &mut mutator,
                size,
                &mut bytes_allocated,
                &mut usable_size,
                &mut bytes_tl_bulk_allocated,
            );
            if mem.is_null() && GC {
                // trigger GC if no memory is available
                return self.alloc_slow(mutator, value);
            } else if mem.is_null() && !GC {
                // if GC hapenned and memory is still unavailbe just OOM
                oom_abort();
            }
            if bytes_tl_bulk_allocated > 0 {
                // update num_bytes_allocated so we can start GC when necessary
                self.num_old_space_allocated
                    .fetch_add(bytes_tl_bulk_allocated, Ordering::Relaxed);
            }

            let header = mem.cast::<HeapObjectHeader>();
            (*header).set_vtable(vtable_of::<T>());
            (*header).set_size(size);
            ((*header).data() as *mut T).write(value);

            Gc {
                base: NonNull::new_unchecked(header),
                marker: PhantomData,
            }
        }
    }

    #[inline]
    fn is_out_of_memory_on_allocation(&self, alloc_size: usize, grow: bool) -> bool {
        let mut old_target = self.next_major_collection_threshold.load(Ordering::Relaxed);
        loop {
            let old_allocated = self.num_old_space_allocated.load(Ordering::Relaxed);
            let new_footprint = old_allocated + alloc_size;
            if new_footprint <= old_target {
                return false;
            } else if new_footprint > self.growth_limit {
                return true;
            }

            if grow {
                if let Err(t) = self.next_major_collection_threshold.compare_exchange_weak(
                    old_target,
                    new_footprint,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    old_target = t;
                    //return false;
                } else {
                    return false;
                }
            } else {
                return true;
            }
        }
    }
}

#[cold]
fn promotion_oom(size: usize, object: *const u8) -> ! {
    eprintln!(
        "Out of memory on young spacep promotion: tried to promote {} at {:p}",
        formatted_size(size),
        object
    );
    std::process::abort()
}

pub struct YoungVisitor<'a> {
    minimark: &'a mut MiniMark,
    parent_object: *mut HeapObjectHeader,
    mutator: &'a mut MutatorRef<MiniMark>,
}

impl<'a> Visitor for YoungVisitor<'a> {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        unsafe {
            self.minimark
                .trace_drag_out(self.mutator, root, self.parent_object);
        }
    }
}

pub struct OldVisitor<'a> {
    minimark: &'a mut MiniMark,
}

impl<'a> Visitor for OldVisitor<'a> {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        unsafe {
            self.minimark.trace(root);
        }
    }
}

impl GcBase for MiniMark {
    type TLAB = MiniMarkTLAB;
    const SUPPORTS_TLAB: bool = true;
    type ReadBarrier = NoReadBarrier;
    const LARGE_ALLOCATION_SIZE: usize = 16 * 1024;
    fn add_constraint<T: crate::gc_base::MarkingConstraint + 'static>(&mut self, constraint: T) {
        self.global_lock();
        self.constraints.push(Box::new(constraint));
        self.global_unlock();
    }
    fn allocate_weak<T: Collectable + ?Sized>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: Gc<T, Self>,
    ) -> Weak<T, Self> {
        let weak_ref = unsafe { Weak::create(mutator, value) };
        self.global_heap_lock.lock();

        self.weak_refs.push(weak_ref.to_dyn());
        unsafe {
            self.global_heap_lock.unlock();
        }
        weak_ref
    }

    /// Generational write barrier implementation. This is "always on" write barrier, this means that if `object` is from old space and not in
    /// remembered set it will be put to remembered set in any case. This write barrier must be used right after write to an object happened.
    #[inline]
    fn write_barrier(
        &mut self,
        _: &mut MutatorRef<Self>,
        object: Gc<dyn crate::api::Collectable, Self>,
    ) {
        unsafe {
            self.write_barrier_internal(object.base.as_ptr());
        }
    }

    fn collect_alloc_failure(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        keep: &mut [&mut dyn Trace],
    ) {
        match SafepointScope::new(mutator.clone()) {
            Some(x) => unsafe {
                self.global_heap_lock.lock();
                self.rem_set_lock.lock();
                self.large_space_lock.lock();
                if self.minor(mutator, keep, GcReason::AllocationFailure) {
                    self.major(mutator, keep, GcReason::OldSpaceFull);
                }
                drop(x);
                self.global_heap_lock.unlock();
                self.rem_set_lock.unlock();
                self.large_space_lock.unlock();
            },
            None => return,
        }
    }
    fn collect(&mut self, mutator: &mut MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        unsafe {
            match SafepointScope::new(mutator.clone()) {
                Some(safepoint) => {
                    self.global_heap_lock.lock();
                    self.rem_set_lock.lock();
                    self.large_space_lock.lock();
                    if self.minor(mutator, keep, GcReason::RequestedByUser) {
                        self.major(mutator, keep, GcReason::OldSpaceFull);
                    }
                    drop(safepoint);
                    self.global_heap_lock.unlock();
                    self.rem_set_lock.unlock();
                    self.large_space_lock.unlock();
                }
                None => return,
            }
        }
    }
    fn full_collection(&mut self, mutator: &mut MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        unsafe {
            let s = mutator.enter_unsafe();
            self.wait_for_gc_to_complete();
            drop(s);
            match SafepointScope::new(mutator.clone()) {
                Some(safepoint) => {
                    self.global_heap_lock.lock();
                    self.rem_set_lock.lock();
                    self.large_space_lock.lock();
                    self.minor(mutator, keep, GcReason::RequestedByUser);
                    self.major(mutator, keep, GcReason::RequestedByUser);
                    drop(safepoint);
                    self.global_heap_lock.unlock();
                    self.rem_set_lock.unlock();
                    self.large_space_lock.unlock();
                }
                None => return,
            }
        }
    }

    fn minor_collection(&mut self, mutator: &mut MutatorRef<Self>, keep: &mut [&mut dyn Trace]) {
        unsafe {
            let s = mutator.enter_unsafe();
            self.wait_for_gc_to_complete();
            drop(s);
            match SafepointScope::new(mutator.clone()) {
                Some(safepoint) => {
                    self.global_heap_lock.lock();
                    self.rem_set_lock.lock();
                    self.large_space_lock.lock();
                    self.minor(mutator, keep, GcReason::RequestedByUser);
                    drop(safepoint);
                    self.global_heap_lock.unlock();
                    self.rem_set_lock.unlock();
                    self.large_space_lock.unlock();
                }
                None => return,
            }
        }
    }

    fn alloc_inline<T: crate::api::Collectable + Sized + 'static>(
        &mut self,
        mutator: &mut MutatorRef<Self>,
        value: T,
        space: AllocationSpace,
    ) -> crate::api::Gc<T, Self> {
        match space {
            AllocationSpace::New => self.alloc_inline_new(mutator, value, space),
            AllocationSpace::Old => self.alloc_inline_old(mutator, value, space),
            _ => unreachable!(),
        }
    }
    #[inline(always)]
    fn post_alloc<T: crate::api::Collectable + Sized + 'static>(&mut self, value: Gc<T, Self>) {
        if std::mem::needs_drop::<T>() {
            unsafe {
                self.finalize_lock.lock();
                if self.nursery.contains(value.base.as_ptr().cast())
                    || !(*PreciseAllocation::from_cell(value.base.as_ptr())).is_marked()
                {
                    self.finalize_list.push_front(value.base.as_ptr());
                } else {
                    self.finalize_list_old.push_front(value.base.as_ptr());
                }

                self.finalize_lock.unlock();
            }
        }
    }
    fn alloc_tlab_area(&mut self, _mutator: &MutatorRef<Self>, _size: usize) -> *mut u8 {
        self.nursery.bump_alloc(32 * 1024)
    }
    fn safepoint(&self) -> &GlobalSafepoint {
        &self.safepoint
    }

    fn attach_current_thread(&mut self, mutator: *mut Mutator<Self>) {
        self.global_heap_lock.lock();
        self.safepoint.n_mutators.fetch_add(1, Ordering::Relaxed);
        self.mutators.push(mutator);
        unsafe { self.global_heap_lock.unlock() };
    }

    fn detach_current_thread(&mut self, mutator: *mut Mutator<Self>) {
        self.global_heap_lock.lock();

        let mut detached = false;
        self.mutators.retain(|x| {
            let x = *x;
            let y = mutator;
            if x == y {
                detached = true;
                false
            } else {
                true
            }
        });
        assert!(detached, "mutator must be detached");
        unsafe {
            self.safepoint.n_mutators.fetch_sub(1, Ordering::Relaxed);

            self.global_heap_lock.unlock();
        }
    }

    fn global_lock(&self) {
        self.global_heap_lock.lock();
    }
    fn global_unlock(&self) {
        unsafe {
            debug_assert!(self.global_heap_lock.is_locked());
            self.global_heap_lock.unlock();
        }
    }
    fn mutators(&self) -> &[*mut Mutator<Self>] {
        assert!(self.global_heap_lock.is_locked());
        &self.mutators
    }
    fn allocate_large<T: crate::api::Collectable + Sized + 'static>(
        &mut self,
        _mutator: &mut MutatorRef<Self>,
        value: T,
    ) -> crate::api::Gc<T, Self> {
        unsafe {
            let size = value.allocation_size() + size_of::<HeapObjectHeader>();
            self.large_space_lock.lock();
            let object = self.large_space.allocate(size);
            (*object).set_vtable(vtable_of::<T>());
            (*object).type_id = small_type_id::<T>();
            let gc = Gc {
                base: NonNull::new_unchecked(object),
                marker: PhantomData,
            };
            ((*object).data() as *mut T).write(value);
            self.large_space_lock.unlock();
            self.post_alloc(gc);
            gc
        }
    }
}

pub struct MiniMarkTLAB {
    heap: Arc<UnsafeCell<MiniMark>>,
    tlab_start: *mut u8,
    tlab_cursor: *mut u8,
    tlab_end: *mut u8,
    // used for promotion
    runs: [*mut Run; NUM_THREAD_LOCAL_SIZE_BRACKETS],
}

impl TLAB<MiniMark> for MiniMarkTLAB {
    fn can_thread_local_allocate(&self, size: usize) -> bool {
        size <= 8 * 1024
    }

    #[inline]
    fn allocate<T: crate::api::Collectable + 'static>(
        &mut self,
        value: T,
    ) -> Result<crate::api::Gc<T, MiniMark>, T> {
        if self.tlab_cursor.is_null() {
            return Err(value);
        }
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        unsafe {
            let result = self.tlab_cursor;
            let new_cursor = result.add(size);
            if new_cursor > self.tlab_end {
                return Err(value);
            }
            self.tlab_cursor = new_cursor;
            let header = result.cast::<HeapObjectHeader>();
            header.write(HeapObjectHeader {
                type_id: small_type_id::<T>(),
                padding: 0,
                padding2: 0,
                value: 0,
            });
            (*header).set_vtable(vtable_of::<T>());
            (*header).set_size(size);
            ((*header).data() as *mut T).write(value);
            let h = &mut *self.heap.get();
            let gc = Gc {
                base: NonNull::new_unchecked(header),
                marker: PhantomData,
            };
            h.post_alloc(gc);
            Ok(gc)
        }
    }

    fn refill(&mut self, mutator: &MutatorRef<MiniMark>, _size: usize) -> bool {
        unsafe {
            let h = &mut *self.heap.get();
            let tlab = h.alloc_tlab_area(mutator, 32 * 1024);
            if tlab.is_null() {
                return false;
            }
            self.tlab_start = tlab;
            self.tlab_end = tlab.add(32 * 1024);
            self.tlab_cursor = tlab;
            true
        }
    }

    fn reset(&mut self) {
        self.tlab_cursor = null_mut();
        self.tlab_end = null_mut();
        self.tlab_start = null_mut();
        self.runs.iter_mut().for_each(|run| {
            *run = dedicated_full_run();
        });
    }
    fn create(heap: Arc<UnsafeCell<MiniMark>>) -> Self {
        Self {
            heap,
            tlab_start: null_mut(),
            tlab_cursor: null_mut(),
            tlab_end: null_mut(),
            runs: [dedicated_full_run(); NUM_THREAD_LOCAL_SIZE_BRACKETS],
        }
    }
}

impl TLABWithRuns for MiniMarkTLAB {
    fn get_runs(&mut self) -> &mut [*mut Run; NUM_THREAD_LOCAL_SIZE_BRACKETS] {
        &mut self.runs
    }
}

fn print_color(c: u8) -> &'static str {
    match c {
        GC_WHITE => "white",
        GC_GREY => "grey",
        GC_BLACK => "black",
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {

    use super::{instantiate_minimark, MiniMarkOptions};

    #[test]
    fn test_weak_refs() {
        let mut options = MiniMarkOptions::default();
        options.nursery_size = 1 * 1024 * 1024;
        options.verbose = true;
        let mut minimark = instantiate_minimark(options);

        let value = minimark.allocate(42, crate::gc_base::AllocationSpace::New);
        letroot!(
            rooted = minimark.shadow_stack(),
            minimark.allocate(44, crate::gc_base::AllocationSpace::New)
        );

        letroot!(
            weak1 = minimark.shadow_stack(),
            minimark.allocate_weak(value)
        );
        letroot!(
            weak2 = minimark.shadow_stack(),
            minimark.allocate_weak(*rooted)
        );

        assert_eq!(*weak1.upgrade().unwrap(), 42);
        assert_eq!(*weak2.upgrade().unwrap(), 44);

        minimark.collect(&mut []);

        assert!(weak1.upgrade().is_none());
        assert_eq!(*weak2.upgrade().unwrap(), 44);
    }
}
