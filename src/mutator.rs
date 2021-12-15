use crate::api::ShadowStack;

/// Thread local allocation buffer size
pub const TLAB_SIZE: usize = 32 * 1024;

/// If object <= TLAB_LARGE_SIZE then it is allocated in TLAB
pub const TLAB_LARGE_SIZE: usize = 8 * 1024;

pub struct Mutator {
    pub thread_local_start: *mut u8,
    pub thread_local_pos: *mut u8,
    pub thread_local_end: *mut u8,
    pub thread_local_limit: *mut u8,
    pub stack: ShadowStack,
}

impl Mutator {
    pub fn tlab_size(&self) -> usize {
        self.thread_local_end as usize - self.thread_local_pos as usize
    }

    pub fn tlab_remaining_capacity(&self) -> usize {
        self.thread_local_limit as usize - self.thread_local_pos as usize
    }

    pub fn expand_tlab(&mut self, bytes: usize) {
        unsafe {
            self.thread_local_end = self.thread_local_end.add(bytes);
            assert!(self.thread_local_end <= self.thread_local_limit);
        }
    }

    pub fn get_tlab_end(&self) -> *mut u8 {
        self.thread_local_end
    }

    pub fn get_tlab_pos(&self) -> *mut u8 {
        self.thread_local_pos
    }

    pub fn get_tlab_limit(&self) -> *mut u8 {
        self.thread_local_limit
    }

    pub fn get_tlab_start(&self) -> *mut u8 {
        self.thread_local_start
    }


}
