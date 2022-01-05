use comet_multi::{
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::AllocationSpace,
    immix, letroot,
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
    let mut mutator = immix::instantiate_immix(
        256 * 1024 * 1024,
        128 * 1024,
        4 * 1024 * 1024,
        128 * 1024 * 1024,
        true,
    );
    let stack = mutator.shadow_stack();
    letroot!(
        x = stack,
        mutator.allocate(Node::None, AllocationSpace::New)
    );

    let mut i = 0;
    while i < 5 * 1024 * 1024 {
        *x = mutator.allocate(
            Node::Some {
                value: 42,
                next: *x,
            },
            AllocationSpace::New,
        );
        i += x.allocation_size();
    }
    *x = mutator.allocate(Node::None, AllocationSpace::New);
    mutator.collect(&mut []);
}
