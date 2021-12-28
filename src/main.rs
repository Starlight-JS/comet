use comet_multi::{
    api::{Collectable, Finalize, Gc, Trace},
    bitmap::SpaceBitmap,
    generational::{self, GenConOptions},
    immix::{Chunk, ImmixBlock, CHUNK_SIZE, IMMIX_BLOCK_SIZE, IMMIX_LINES_PER_BLOCK},
    letroot,
    utils::{formatted_size, mmap::Mmap},
};

pub enum Node {
    None,
    Some { value: i64, next: Gc<Node> },
}

unsafe impl Trace for Node {
    fn trace(&mut self, vis: &mut dyn comet_multi::api::Visitor) {
        match self {
            Self::Some { next, .. } => {
                next.trace(vis);
            }
            _ => (),
        }
    }
}

unsafe impl Finalize for Node {}
impl Collectable for Node {}

fn main() {
    unsafe {
        let mem = Mmap::new(CHUNK_SIZE * 10, IMMIX_BLOCK_SIZE);

        let chunk = Chunk::new(mem.aligned_start());
        println!("{:p}", chunk);
        println!("{:p}", (*chunk).blocks_start());
    }
}
