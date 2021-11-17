use crate::block::Block;
use crossbeam_utils::atomic::AtomicCell;
use std::{ptr::null_mut, sync::atomic::AtomicUsize};
#[derive(Clone)]
pub struct AllBlockList {
    head: *mut Block,
}

impl AllBlockList {
    pub fn new() -> Self {
        Self { head: null_mut() }
    }

    pub fn push(&mut self, block: *mut Block) {
        unsafe {
            (*block).all_next = self.head;
            self.head = block;
        }
    }

    pub fn pop(&mut self) -> *mut Block {
        unsafe {
            if self.head.is_null() {
                return null_mut();
            }
            let head = self.head;
            self.head = (*head).all_next;
            head
        }
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }
}

#[derive(Clone)]
pub struct BlockList {
    head: *mut Block,
}

impl BlockList {
    pub fn new() -> Self {
        Self { head: null_mut() }
    }
    pub fn len(&self) -> usize {
        let mut c = 0;
        self.for_each(|_| c += 1);
        c
    }
    pub fn for_each(&self, mut visitor: impl FnMut(*mut Block)) {
        unsafe {
            let mut head = self.head;
            while !head.is_null() {
                visitor(head);
                head = (*head).next;
            }
        }
    }
    pub fn push(&mut self, block: *mut Block) {
        unsafe {
            (*block).next = self.head;
            self.head = block;
        }
    }

    pub fn pop(&mut self) -> *mut Block {
        unsafe {
            if self.head.is_null() {
                return null_mut();
            }
            let head = self.head;
            self.head = (*head).next;
            head
        }
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }
}

/// Lock-free block list
pub struct AtomicBlockList {
    next: AtomicCell<*mut Block>,
    count: AtomicUsize,
}
impl Clone for AtomicBlockList {
    fn clone(&self) -> Self {
        Self {
            next: AtomicCell::new(null_mut()),
            count: AtomicUsize::new(0),
        }
    }
}
impl AtomicBlockList {
    pub fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            next: AtomicCell::new(null_mut()),
        }
    }
    pub fn head(&self) -> *mut Block {
        self.next.load()
    }
    pub unsafe fn add_free(&self, free: *mut Block) {
        let new_slot = free;
        let mut next = self.next.load();
        loop {
            debug_assert_ne!(new_slot, next);
            (*new_slot).next = next;
            match self.next.compare_exchange(next, new_slot) {
                Ok(_) => {
                    self.count.fetch_add(1, atomic::Ordering::AcqRel);
                    return;
                }
                Err(actual_next) => {
                    next = actual_next;
                }
            }
        }
    }
    #[inline]
    pub fn take_free(&self) -> *mut Block {
        loop {
            unsafe {
                let next_free = match self.next.load() {
                    x if x.is_null() => return null_mut(),
                    x => x,
                };
                debug_assert_ne!(next_free, (*next_free).next);
                if self
                    .next
                    .compare_exchange(next_free, (*next_free).next)
                    .is_err()
                {
                    continue;
                }
                self.count.fetch_sub(1, atomic::Ordering::AcqRel);
                return next_free;
            }
        }
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.count.load(atomic::Ordering::Acquire)
    }
}
