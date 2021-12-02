use std::{
    intrinsics::unlikely,
    mem::size_of,
    ptr::{null_mut, NonNull},
};

use crate::{
    api::{
        vtable_of, Collectable, Gc, HeapObjectHeader, ShadowStack, Trace, Visitor, MIN_ALLOCATION,
    },
    base::GcBase,
    bump_pointer_space::{align_usize, BumpPointerSpace},
    large_space::{LargeObjectSpace, PreciseAllocation},
};
use im::Vector;

/// SemiSpace collector. It divides heap into two spaces: from_space and to_space.
///
/// All allocations go to `from_space` and when it is full, GC is triggered and all surviving
/// objects from `from_space` go to `to_space` and at the end of GC cycle these spaces are swapped.
///
///
/// ## Large objects
/// Large objects (>=64kb) use malloc directly and there is no threshold for triggering GC based on large object
/// space so if your program allocates too much large objects be sure to sometimes trigger collection manually.
#[repr(C)]
pub struct SemiSpace {
    shadow_stack: ShadowStack,
    from_space: BumpPointerSpace,
    to_space: BumpPointerSpace,
    objects_with_finalizers: Vector<*mut HeapObjectHeader>,
    finalize_lock: bool,

    large: LargeObjectSpace,
    mark_stack: Vec<*mut HeapObjectHeader>,
    objects_moved: usize,
    total_allocations: usize,
    num_allocated_since_last_gc: usize,
}
pub fn get_forwarding_address_in_from_space(obj: *const HeapObjectHeader) -> *mut HeapObjectHeader {
    unsafe {
        if !(*obj).is_forwarded() {
            return null_mut();
        }
        (*obj).vtable() as _
    }
}

const VERBOSE: bool = !true;

impl SemiSpace {
    pub fn new(space_size: usize) -> Box<SemiSpace> {
        let from_space = BumpPointerSpace::create("bump pointer 1", space_size);
        let to_space = BumpPointerSpace::create("bump pointer 2", space_size);

        Box::new(Self {
            from_space,
            to_space,
            objects_with_finalizers: Vector::new(),
            finalize_lock: false,
            shadow_stack: ShadowStack::new(),
            large: LargeObjectSpace::new(),
            mark_stack: Vec::with_capacity(128),
            objects_moved: 0,
            total_allocations: 0,
            num_allocated_since_last_gc: 0,
        })
    }

    fn mark_non_forwarded_object(&mut self, obj: *const HeapObjectHeader) -> *mut HeapObjectHeader {
        unsafe {
            let object_size = (*obj).size();
            let forward_address = self
                .to_space
                .alloc_thread_unsafe(object_size, &mut 0, &mut 0);

            if forward_address.is_null() {
                panic!("Out of memory in the to-space");
            }
            if VERBOSE {
                self.objects_moved += 1;
            }
            std::ptr::copy_nonoverlapping(
                obj.cast::<u8>(),
                forward_address.cast::<u8>(),
                object_size,
            );
            forward_address
        }
    }

    fn mark_object_(&mut self, root: &mut std::ptr::NonNull<HeapObjectHeader>) {
        unsafe {
            let obj = *root;
            let obj = obj.as_ptr();
            if self.from_space.has_address(obj) {
                let mut forward_address = get_forwarding_address_in_from_space(obj);
                if forward_address.is_null() {
                    forward_address = self.mark_non_forwarded_object(obj);
                    (*obj).set_forwarded(forward_address as _);
                    self.mark_stack.push(forward_address);
                }
                *root = NonNull::new_unchecked(forward_address);
            } else {
                if (*obj).is_precise() {
                    let large = &mut *PreciseAllocation::from_cell(obj);
                    if !large.is_marked() {
                        large.mark = true;
                        self.mark_stack.push(obj);
                    }
                }
            }
        }
    }

    fn deal_with_finalizers(&mut self) {
        let mut new_vec = Vector::new();

        while let Some(object) = self.objects_with_finalizers.pop_back() {
            unsafe {
                if (*object).is_forwarded() {
                    new_vec.push_front(get_forwarding_address_in_from_space(object));
                } else if (*object).is_precise()
                    && (*PreciseAllocation::from_cell(object)).is_marked()
                {
                    new_vec.push_front(object);
                } else {
                    let object = (*object).get_dyn();
                    object.finalize();
                }
            }
        }
    }
    pub fn total_allocations(&self) -> usize {
        self.total_allocations
    }

    #[cold]
    fn allocate_with_gc<T: Collectable + 'static>(&mut self, mut value: T) -> Gc<T> {
        // collect memory and keep value alive.
        self.collect(&mut [&mut value]);

        match self.try_allocate(value) {
            Ok(val) => val,
            Err(_) => {
                eprintln!("FATAL: Out of memory");
                std::process::abort();
            }
        }
    }
}

impl Visitor for SemiSpace {
    fn mark_object(&mut self, root: &mut std::ptr::NonNull<HeapObjectHeader>) {
        if !self.to_space.has_address(root.as_ptr()) {
            self.mark_object_(root);
        }
    }
}

impl GcBase for SemiSpace {
    fn collect(&mut self, references: &mut [&mut dyn Trace]) {
        unsafe {
            let to_mmap = self.to_space.get_mem_map();
            to_mmap.commit(to_mmap.start(), to_mmap.size());

            let stack = self.shadow_stack();
            stack.walk(|entry| {
                entry.trace(self);
            });

            for var in references {
                var.trace(self);
            }

            while let Some(object) = self.mark_stack.pop() {
                (*object).get_dyn().trace(self);
            }
            self.deal_with_finalizers();
            self.large.sweep();
            self.large.prepare_for_allocation(false);
            //self.from_space.set_end(self.from_space.begin());
            self.from_space.clear();
            std::mem::swap(&mut self.from_space, &mut self.to_space);
            if VERBOSE {
                println!(
                    "GC end; Objects moved: {}; allocated objects since last GC: {}",
                    self.objects_moved, self.num_allocated_since_last_gc
                );
                self.objects_moved = 0;
                self.num_allocated_since_last_gc = 0;
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
    fn try_allocate<T: Collectable + 'static>(&mut self, value: T) -> Result<Gc<T>, T> {
        let size = align_usize(
            value.allocation_size() + size_of::<HeapObjectHeader>(),
            MIN_ALLOCATION,
        );
        unsafe {
            let memory = self.from_space.alloc_thread_unsafe(size, &mut 0, &mut 0);
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
            (*memory).set_size(size);
            ((*memory).data() as *mut T).write(value);
            if std::mem::needs_drop::<T>() {
                self.objects_with_finalizers.push_back(memory);
            }
            // self.num_allocated_since_last_gc += 1;
            Ok(Gc {
                base: NonNull::new_unchecked(memory),
                marker: Default::default(),
            })
        }
    }

    #[inline(always)]
    fn allocate<T: Collectable + 'static>(&mut self, value: T) -> Gc<T> {
        match self.try_allocate(value) {
            Ok(val) => val,
            Err(val) => self.allocate_with_gc(val),
        }
    }
    fn finalize_handlers(&self) -> &Vector<*mut HeapObjectHeader> {
        &self.objects_with_finalizers
    }

    fn finalize_handlers_mut(&mut self) -> &mut Vector<*mut HeapObjectHeader> {
        &mut self.objects_with_finalizers
    }

    fn finalize_lock(&self) -> bool {
        self.finalize_lock
    }
    fn set_finalize_lock(&mut self, x: bool) {
        self.finalize_lock = x;
    }
    #[inline(always)]
    fn shadow_stack<'a>(&self) -> &'a ShadowStack {
        unsafe { std::mem::transmute(&self.shadow_stack) }
    }
}
