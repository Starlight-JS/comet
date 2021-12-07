use comet::alloc::fixed_array::FixedArray;

use comet::alloc::string::GcStr;
use comet::alloc::vector::Vector;
use comet::api::{Collectable, Field, Finalize, Gc, Trace};
use comet::base::GcBase;
use comet::letroot;
use comet::minimark::MiniMarkGC;

enum Node {
    None,
    Some { value: i64, next: Gc<Node> },
}

unsafe impl Trace for Node {
    fn trace(&mut self, _vis: &mut dyn comet::api::Visitor) {
        match self {
            Self::Some { next, .. } => {
                next.trace(_vis);
            }
            _ => (),
        }
    }
}

unsafe impl Finalize for Node {}

impl Collectable for Node {}
fn main() {
    let mut heap = MiniMarkGC::new(Some(4 * 1024 * 1024), None, None);
    let stack = heap.shadow_stack();
    letroot!(vec = stack, Vector::<u8>::with_capacity(&mut *heap, 8));
    vec.push_back(&mut *heap, 42);
    vec.push_back(&mut *heap, 44);
    vec.insert(&mut *heap, 1, 43);

    //println!("{:p}", &vec[0])
}
