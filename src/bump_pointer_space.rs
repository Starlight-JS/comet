use std::{ptr::null_mut, sync::atomic::AtomicPtr};

use crate::utils::mmap::Mmap;

pub struct BumpPointerSpace {
    mmap: Mmap,
    start: *mut u8,
    end: *mut u8,
    cursor: AtomicPtr<u8>,
}

impl BumpPointerSpace {
    pub fn contains(&self, addr: *const u8) -> bool {
        addr >= self.start && addr < self.end
    }
    pub fn new(size: usize) -> Self {
        let mmap = Mmap::new(size);
        let start = mmap.start();
        let end = mmap.end();
        let cursor = AtomicPtr::new(start);
        let this = Self {
            mmap,
            start,
            end,
            cursor,
        };

        this
    }

    pub fn reset(&self) {
        self.cursor.store(self.start, atomic::Ordering::Relaxed);
    }

    pub fn decommit(&self) {
        unsafe {
            self.mmap
                .decommit(self.start, self.end.offset_from(self.start) as _);
        }
    }

    pub fn commit(&self) {
        unsafe {
            self.mmap
                .commit(self.start, self.end.offset_from(self.start) as _);
        }
    }
    #[inline]
    pub fn bump_alloc(&self, size: usize) -> *mut u8 {
        let mut old = self.cursor.load(atomic::Ordering::Relaxed);
        let mut new;
        loop {
            unsafe {
                new = old.add(size);
                if new > self.end {
                    return null_mut();
                }

                let res = self.cursor.compare_exchange_weak(
                    old,
                    new,
                    atomic::Ordering::SeqCst,
                    atomic::Ordering::Relaxed,
                );
                match res {
                    Ok(_) => break,
                    Err(x) => old = x,
                }
            }
        }
        old
    }

    pub unsafe fn thread_bump_alloc_unsafe(&self, size: usize) -> *mut u8 {
        let old = self.cursor.load(atomic::Ordering::Relaxed);

        let new = old.add(size);
        if new > self.end {
            return null_mut();
        }
        self.cursor.store(new, atomic::Ordering::Release);

        old
    }
}
