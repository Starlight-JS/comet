use comet_multi::{
    api::{Collectable, Finalize, Gc, Trace},
    immix, letroot,
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
    let opts = ShenandoahHeapRegion::setup_sizes(4 * 1024 * 1024, None, None, None);
    println!("{:?}", opts);
    /*let mut immix = immix::instantiate_immix(
        1024 * 1024 * 1024,
        64 * 1024 * 1024,
        136 * 1024 * 1024,
        256 * 1024 * 1024,
        true,
    );
    let stack = immix.shadow_stack();
    letroot!(list = stack, immix.allocate(Node::None));
    let start = std::time::Instant::now();
    let mut i = 0;
    while i < 500_000_000 {
        *list = immix.allocate(Node::Some {
            value: 42,
            next: *list,
        });
        if i % 10000 == 0 {
            *list = immix.allocate(Node::None);
        }
        i += 1;
    }

    println!("{:.4} seconds", start.elapsed().as_secs_f64());*/
}
