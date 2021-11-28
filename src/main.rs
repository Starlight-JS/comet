use semispace::api::Collectable;
use semispace::api::Field;
use semispace::api::Trace;
use semispace::letroot;
use semispace::Heap;

enum Node {
    None,
    Next { value: i64, next: Field<Self> },
}

impl Trace for Node {
    fn trace(&mut self, vis: &mut dyn semispace::api::Visitor) {
        match self {
            Self::None => {}
            Self::Next { next, .. } => next.trace(vis),
        }
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "[]"),
            Self::Next { value, next } => {
                write!(f, "{} :: {:?}", value, **next)
            }
        }
    }
}

impl Collectable for Node {}
/*impl Drop for Node {
    fn drop(&mut self) {
        println!("Drop {:p}", self);
    }
}*/
fn main() {
    let mut heap = Heap::new(1 * 1024 * 1024);
    let stack = heap.shadow_stack();
    let start = std::time::Instant::now();
    letroot!(l = stack, heap.allocate_with_gc(Node::None));

    let mut i = 0;
    while i < 500_000_000 {
        *l = heap.allocate_with_gc(Node::Next {
            value: 42,
            next: l.to_field(),
        });
        if i % 8192 == 0 {
            *l = heap.allocate_with_gc(Node::None);
        }
        i += 1;
    }

    let end = start.elapsed();

    let total = 500_000_000 * 32usize;
    println!(
        "Allocated ~{:.2}mb in {}s, throughput=~{}Gbps",
        total as f64 / 1024.0 / 1024.0,
        end.as_secs(),
        (total as f64 / 1024.0 / 1024.0 / 1024.0) / end.as_secs() as f64
    )
}
