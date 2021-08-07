pub const IMMIX_BLOCK_SIZE: usize = 32 * 1024;
pub const LINE_SIZE: usize = 256;
pub const LINE_COUNT: usize = IMMIX_BLOCK_SIZE / LINE_SIZE; // first line is occupied by the block header.
pub const LARGE_CUTOFF: usize = IMMIX_BLOCK_SIZE / 4;
/// Objects larger than medium cutoff span multiple lines and require special Overflow allocator
pub const MEDIUM_CUTOFF: usize = LINE_SIZE;
