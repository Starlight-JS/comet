use comet::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::{AllocationSpace, GcBase},
    letroot,
    minimark::{instantiate_minimark, MiniMarkOptions},
};

pub enum Node<H: GcBase> {
    None,
    Some { value: i64, next: Gc<Node<H>, H> },
}

unsafe impl<H: GcBase> Trace for Node<H> {
    fn trace(&mut self, vis: &mut dyn comet::api::Visitor) {
        match self {
            Self::Some { next, .. } => {
                next.trace(vis);
            }
            _ => (),
        }
    }
}

unsafe impl<H: GcBase> Finalize for Node<H> {}
impl<H: GcBase> Collectable for Node<H> {}

fn main() {
    let mut opts = MiniMarkOptions::default();
    opts.verbose = true;

    let mut mutator = instantiate_minimark(opts);
    let start = std::time::Instant::now();
    let stack = mutator.shadow_stack();
    letroot!(
        list = stack,
        mutator.allocate(Node::None, AllocationSpace::New)
    );

    let mut i = 0;
    while i < 500_000_000 {
        *list = mutator.allocate(
            Node::Some {
                next: *list,
                value: 42,
            },
            AllocationSpace::New,
        );

        if i % 8192 == 0 {
            *list = mutator.allocate(Node::None, AllocationSpace::New);
        }
        i += 1;
    }

    println!("Finished in {:.4} secs", start.elapsed().as_secs_f64());
}
