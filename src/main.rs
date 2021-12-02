use comet::api::{Collectable, Field, Finalize, Trace};
use comet::base::GcBase;
use comet::letroot;
use comet::minimark::MiniMarkGC;

enum Node {
    None,
    Some { _value: i64, next: Field<Node> },
}

impl Collectable for Node {}
unsafe impl Trace for Node {
    fn trace(&mut self, _vis: &mut dyn comet::api::Visitor) {
        match self {
            Self::Some { next, .. } => next.trace(_vis),
            _ => (),
        }
    }
}

unsafe impl Finalize for Node {}

fn main() {
    let mut heap = MiniMarkGC::new(Some(256 * 1024 * 1024), None, None);

    let stack = heap.shadow_stack();
    let start = std::time::Instant::now();
    letroot!(l = stack, heap.allocate(Node::None));
    let mut i = 0;
    while i < 500_000_000 {
        letroot!(tmp = stack, *l);
        *l = heap.allocate(Node::Some {
            _value: 0,
            next: tmp.to_field(),
        });
        if i % 8129 == 0 {
            *l = heap.allocate(Node::None);
        }
        i += 1;
    }

    let end = start.elapsed();
    println!("Complete in {}", end.as_millis());
}
