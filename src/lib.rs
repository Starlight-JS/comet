#[macro_use]
pub mod util;
#[macro_use]
pub mod api;
pub mod bitmap;
use std::{
    any::TypeId,
    marker::PhantomData,
    mem::size_of,
    ptr::{null_mut, NonNull},
};

use api::{
    vtable_of, Collectable, Gc, HeapObjectHeader, ShadowStack, Trace, Visitor, MIN_ALLOCATION,
};
use bitmap::{HeapBitmap, SpaceBitmap};
use bump_pointer_space::{align_usize, BumpPointerSpace};
pub use mopa;
pub mod bump_pointer_space;
pub mod space;

pub struct Heap {
    temp_space: BumpPointerSpace,
    bump_pointer_space: BumpPointerSpace,
    mark_bitmap: HeapBitmap,
    stack: ShadowStack,
    total_allocated: usize,
    cached_live_bitmap: *const SpaceBitmap<{ MIN_ALLOCATION }>,
}

impl Heap {
    pub fn new(capacity: usize) -> Box<Self> {
        let mut temp_space = BumpPointerSpace::create("temp-space", capacity);
        let mut bump_pointer_space = BumpPointerSpace::create("bump-pointer-space", capacity);
        temp_space.bind_bitmaps();
        bump_pointer_space.bind_bitmaps();
        let mark_bitmap = HeapBitmap::new();

        let mut this = Box::new(Self {
            cached_live_bitmap: null_mut() as *const _,
            temp_space,
            bump_pointer_space,
            mark_bitmap,
            total_allocated: 0,
            stack: ShadowStack::new(),
        });

        this.cached_live_bitmap = this.bump_pointer_space.get_live_bitmap();
        this.mark_bitmap
            .add_continuous_space(this.temp_space.get_mark_bitmap());
        this.mark_bitmap
            .add_continuous_space(this.bump_pointer_space.get_mark_bitmap());
        this
    }

    pub fn swap_semispaces(&mut self) {
        std::mem::swap(&mut self.bump_pointer_space, &mut self.temp_space);
        self.cached_live_bitmap = self.bump_pointer_space.get_live_bitmap();
    }
    pub fn total_allocated(&self) -> usize {
        self.total_allocated
    }
    #[inline(always)]
    pub fn try_allocate<T: Collectable + 'static>(&mut self, value: T) -> Result<Gc<T>, T> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);
        let ptr = self
            .bump_pointer_space
            .alloc_non_virtual_without_accounting(size);

        if ptr.is_null() {
            return Err(value);
        }

        unsafe {
            // self.total_allocated += size;
            ptr.write(HeapObjectHeader {
                type_id: TypeId::of::<T>(),
                value: 0,
            });
            (*ptr).set_vtable(vtable_of::<T>());
            (*ptr).set_size(size);
            if std::mem::needs_drop::<T>() {
                self.bump_pointer_space.get_finalize_bitmap().set(ptr as _);
            }
            ((*ptr).data() as *mut T).write(value);
            (*self.cached_live_bitmap).set(ptr as _);
            Ok(Gc {
                base: NonNull::new_unchecked(ptr),
                marker: PhantomData,
            })
        }
    }

    #[inline(always)]
    pub fn allocate_with_gc<T: Collectable + 'static + std::marker::Unpin>(
        &mut self,
        value: T,
    ) -> Gc<T> {
        let size = align_usize(value.allocation_size() + size_of::<HeapObjectHeader>(), 8);

        let ptr = self
            .bump_pointer_space
            .alloc_non_virtual_without_accounting(size);

        if !ptr.is_null() {
            unsafe {
                // self.total_allocated += size;
                ptr.write(HeapObjectHeader {
                    type_id: TypeId::of::<T>(),
                    value: 0,
                });
                (*ptr).set_vtable(vtable_of::<T>());
                (*ptr).set_size(size);
                if std::mem::needs_drop::<T>() {
                    self.bump_pointer_space.get_finalize_bitmap().set(ptr as _);
                }
                ((*ptr).data() as *mut T).write(value);

                (*self.cached_live_bitmap).set(ptr as _);
                return Gc {
                    base: NonNull::new_unchecked(ptr),
                    marker: PhantomData,
                };
            }
        }

        self.allocate_slow(value)
    }
    pub fn shadow_stack(&self) -> &'static ShadowStack {
        unsafe { std::mem::transmute(&self.stack) }
    }
    #[cold]
    fn allocate_slow<T: Collectable + 'static + std::marker::Unpin>(
        &mut self,
        mut value: T,
    ) -> Gc<T> {
        for _ in 0..3 {
            self.gc(&mut [&mut value]);

            match self.try_allocate(value) {
                Ok(val) => return val,
                Err(e) => value = e,
            }
        }
        eprintln!("FATAL: Out of memory");
        std::process::abort();
    }

    pub fn gc(&mut self, references: &mut [&mut dyn Trace]) {
        let mut semispace = SemiSpace {
            to_space_live_bitmap: null_mut(),
            mark_bitmap: null_mut(),
            to_space: &mut self.temp_space,
            from_space: &mut self.bump_pointer_space,
            objects_moved: 0,
            bytes_moved: 0,
            saved_bytes: 0,
            mark_stack: Vec::with_capacity(128),
            shadow_stack: &self.stack,
            heap: self as *mut Self,
        };
        semispace.run(references);
    }
}

const VERBOSE: bool = false;

#[allow(dead_code)]
pub struct SemiSpace {
    bytes_moved: usize,
    objects_moved: usize,
    saved_bytes: usize,
    to_space: *mut BumpPointerSpace,
    to_space_live_bitmap: *mut SpaceBitmap<{ MIN_ALLOCATION }>,
    from_space: *mut BumpPointerSpace,
    mark_bitmap: *mut HeapBitmap,
    mark_stack: Vec<*mut HeapObjectHeader>,
    shadow_stack: *const ShadowStack,
    heap: *mut Heap,
}

impl SemiSpace {
    pub fn get_forwarding_address_in_from_space(
        &self,
        obj: *const HeapObjectHeader,
    ) -> *mut HeapObjectHeader {
        unsafe {
            if !(*obj).is_forwarded() {
                return null_mut();
            }
            (*obj).vtable() as _
        }
    }

    pub fn mark_non_forwarded_object(
        &mut self,
        obj: *const HeapObjectHeader,
    ) -> *mut HeapObjectHeader {
        unsafe {
            let object_size = (*obj).size();

            let mut bytes_allocated = 0;
            let mut dummy = 0;

            let forward_address =
                (*self.to_space).alloc_thread_unsafe(object_size, &mut bytes_allocated, &mut dummy);
            if !forward_address.is_null() && !self.to_space_live_bitmap.is_null() {
                (*self.to_space_live_bitmap).set(forward_address as _);
            }

            if forward_address.is_null() {
                panic!("Out of memory in the to-space");
            }
            self.saved_bytes +=
                self.copy_avoiding_dirtying_pages(forward_address.cast(), obj.cast(), object_size);
            self.bytes_moved += bytes_allocated;
            forward_address
        }
    }

    unsafe fn copy_avoiding_dirtying_pages(
        &mut self,
        dest: *mut u8,
        src: *const u8,
        size: usize,
    ) -> usize {
        //if size <= 4096 {
        std::ptr::copy_nonoverlapping(src, dest, size);
        for i in 0..size {
            assert_eq!(src.add(i).read(), dest.add(i).read());
        }
        // }

        0
    }

    fn mark_object_if_not_in_to_space(&mut self, root: &mut std::ptr::NonNull<HeapObjectHeader>) {
        unsafe {
            if !(*self.to_space).has_address(root.as_ptr()) {
                self.mark_object_(root);
            }
        }
    }

    fn mark_object_(&mut self, root: &mut std::ptr::NonNull<HeapObjectHeader>) {
        unsafe {
            let obj = *root;
            let obj = obj.as_ptr();
            if (*self.from_space).has_address(obj) {
                let mut forward_address = self.get_forwarding_address_in_from_space(obj);
                if forward_address.is_null() {
                    forward_address = self.mark_non_forwarded_object(root.as_ptr());

                    (*obj).set_forwarded(forward_address as _);

                    self.mark_stack.push(forward_address);
                }
                if VERBOSE {
                    println!("Copy {:p}->{:p}", obj, forward_address);
                }
                *root = NonNull::new_unchecked(forward_address);
            } else {
                if !(*self.mark_bitmap).set(obj) {
                    self.mark_stack.push(obj);
                }
                // TODO: Mark large allocation
            }
        }
    }

    pub fn marking_phase(&mut self, references: &mut [&mut dyn Trace]) {
        self.bind_bitmaps();
        unsafe {
            (*self.shadow_stack).walk(|object| {
                object.trace(self);
            });
            for ref_ in references {
                ref_.trace(self);
            }
        }
        self.mark_reachable_objects();

        unsafe {
            let finalize = (*self.from_space).get_finalize_bitmap();
            finalize.visit_marked_range(
                finalize.heap_begin() as _,
                (*self.from_space).end() as _,
                |object| {
                    finalize.clear(object as _);

                    if !(*object).is_forwarded() {
                        std::ptr::drop_in_place((*object).get_dyn());
                    }
                },
            );

            (*self.from_space).clear();
            (*self.heap).swap_semispaces();
        }
    }

    pub fn run(&mut self, references: &mut [&mut dyn Trace]) {
        unsafe {
            (*self.to_space).get_mem_map().commit(
                (*self.to_space).get_mem_map().start(),
                (*self.to_space).get_mem_map().size(),
            );
            self.to_space_live_bitmap = (*self.to_space).get_live_bitmap_mut();
            self.mark_bitmap = &mut (*self.heap).mark_bitmap;
        }
        self.marking_phase(references);
        self.reclaim_phase();

        unsafe {
            if !(*self.to_space).get_live_bitmap().is_null()
                && !(*self.to_space).has_bound_bitmaps()
            {
                (*self.to_space).get_mark_bitmap_mut().clear_all();
            }

            if !(*self.from_space).get_live_bitmap().is_null()
                && !(*self.from_space).has_bound_bitmaps()
            {
                (*self.from_space).get_mark_bitmap_mut().clear_all();
            }
        }
    }

    pub fn reclaim_phase(&mut self) {
        self.sweep(false);

        self.swap_bitmaps();

        self.unbind_bitmaps();
    }

    pub fn mark_reachable_objects(&mut self) {
        while let Some(object) = self.mark_stack.pop() {
            unsafe {
                (*object).get_dyn().trace(self);
            }
        }
    }

    pub fn swap_bitmaps(&mut self) {
        unsafe {
            if !(*self.to_space).get_live_bitmap().is_null()
                && !(*self.to_space).has_bound_bitmaps()
            {
                (*self.to_space).swap_bitmaps();
            }
            if !(*self.from_space).get_live_bitmap().is_null()
                && !(*self.from_space).has_bound_bitmaps()
            {
                (*self.from_space).swap_bitmaps();
            }
        }
    }

    pub fn sweep(&mut self, _swap_bitmaps: bool) {}

    pub fn bind_bitmaps(&mut self) {
        unsafe {
            (*self.to_space).bind_live_to_mark_bitmap();
        }
    }

    pub fn unbind_bitmaps(&mut self) {
        unsafe {
            if !(*self.to_space).get_live_bitmap().is_null() && (*self.to_space).has_bound_bitmaps()
            {
                (*self.to_space).unbind_bitmaps();
            }
            if !(*self.from_space).get_live_bitmap().is_null()
                && (*self.from_space).has_bound_bitmaps()
            {
                (*self.from_space).unbind_bitmaps();
            }
        }
    }
}

impl Visitor for SemiSpace {
    fn mark_object(&mut self, root: &mut NonNull<HeapObjectHeader>) {
        self.mark_object_if_not_in_to_space(root);
    }
}
