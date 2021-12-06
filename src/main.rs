use comet::alloc::fixed_array::FixedArray;

use comet::alloc::string::GcStr;
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
    let start = std::time::Instant::now();
    letroot!(list = stack, heap.allocate(Node::None));
    let mut i = 0;
    while i < 500_000_000 {
        *list = heap.allocate(Node::Some {
            value: 42,
            next: *list,
        });

        if i % 8192 == 0 {
            *list = heap.allocate(Node::None);
        }
        i += 1;
    }
    println!("{:.2}", heap.old_space_allocated() as f64 / 1024.0 / 1024.0);
    println!("Done {} {}ms", i, start.elapsed().as_millis());
}
