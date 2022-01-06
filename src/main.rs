use comet_multi::{
    alloc::vector::Vector,
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::AllocationSpace,
    immix, letroot,
    minimark::{instantiate_minimark, MiniMarkOptions},
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
    let mut options = MiniMarkOptions::default();
    options.nursery_size = 1 * 1024 * 1024;
    options.verbose = true;
    let mut minimark = instantiate_minimark(options);
    let stack = minimark.shadow_stack();
    letroot!(
        x = stack,
        Vector::<Gc<i32>>::with_capacity(&mut minimark, 4)
    );

    minimark.minor_collection(&mut []);

    let y = minimark.allocate(42, AllocationSpace::New);
    x.push(&mut minimark, y);
    x.write_barrier(&mut minimark);
    minimark.full_collection(&mut []);

    assert_eq!(**x.at(0), 42);

    *x = Vector::<Gc<i32>>::new(&mut minimark);
    minimark.full_collection(&mut []);
}
