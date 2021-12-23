pub const IMMIX_BLOCK_SIZE: usize = 32 * 1024;
pub const IMMIX_LINE_SIZE: usize = 256;
pub const IMMIX_LINES_PER_BLOCK: usize = IMMIX_BLOCK_SIZE / IMMIX_LINE_SIZE;

pub struct ImmixBlock {
    hole_count: u32,
}

impl ImmixBlock {
    pub fn from_object(object: *const u8) -> *mut Self {
        unsafe {
            let offset = object as usize % IMMIX_BLOCK_SIZE;
            let block = object.offset(-(offset as isize));
            block as *mut Self
        }
    }
}
