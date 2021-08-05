use std::ptr::null_mut;

use crate::{
    block::Block,
    global_allocator::{size_class_to_index, GlobalAllocator},
    heap::Heap,
    internal::block_list::BlockList,
    local_heap::LocalHeap,
};

#[derive(Clone)]
pub struct LocalAllocator {
    pub cell_size: u16,
    pub current_block: *mut Block,
    /// List of unavailable blocks. It is retained when collection starts.
    pub unavailable: BlockList,
    local_heap: *mut LocalHeap,
    global: *mut GlobalAllocator,
}

impl LocalAllocator {
    pub fn allocate(&mut self) -> (*mut u8, usize) {
        if self.current_block.is_null() {
            return (self.allocate_slow(), self.cell_size as _);
        }
        unsafe {
            let mem = (*self.current_block).allocate();
            if mem.is_null() {
                return (self.allocate_slow(), self.cell_size as _);
            }
            (mem, self.cell_size as _)
        }
    }

    #[cold]
    fn allocate_slow(&mut self) -> *mut u8 {
        unsafe {
            if !self.current_block.is_null() {
                self.unavailable.push(self.current_block);
            }
            let block = (*self.global).acquire_block(size_class_to_index(self.cell_size as _));
            if block.is_null() {
                return null_mut();
            }
            self.current_block = block;
            (*block).allocate()
        }
    }

    pub fn new(
        local: *mut LocalHeap,
        _heap: *mut Heap,
        global: *mut GlobalAllocator,
        index: usize,
    ) -> Self {
        let size = unsafe { (*global).size_class_for_size_step[index] };
        assert_ne!(size, 0);
        Self {
            current_block: null_mut(),
            cell_size: size as _,
            local_heap: local,
            global,
            unavailable: BlockList::new(),
        }
    }
}
