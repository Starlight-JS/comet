use comet_multi::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::AllocationSpace,
    immix, letroot, marksweep,
    safepoint::verbose_safepoint,
    shenandoah::region::ShenandoahHeapRegion,
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
    let mut heap = immix::instantiate_immix(
        1024 * 1024 * 1024,
        64 * 1024 * 1024,
        128 * 1024 * 1024,
        512 * 1024 * 1024,
        true,
    );
    let start = std::time::Instant::now();
    let stack = heap.shadow_stack();
    letroot!(
        list = stack,
        heap.allocate(Node::None, AllocationSpace::New)
    );

    let mut i = 0;
    while i < 500_000_000 {
        *list = heap.allocate(
            Node::Some {
                value: 42,
                next: *list,
            },
            AllocationSpace::New,
        );
        if i % 10000 == 0 {
            *list = heap.allocate(Node::None, AllocationSpace::New);
            if heap.safepoint() {
                println!("Safepoint reached");
            }
        }
        i += 1;
    }

    println!("{:.4} seconds", start.elapsed().as_secs_f64());
}
