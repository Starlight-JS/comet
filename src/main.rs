use comet_multi::{
    alloc::array::Array,
    api::{Collectable, Finalize, Gc, Trace},
    gc_base::AllocationSpace,
    generational::{instantiate_gencon, GenConOptions},
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
    let mut heap = immix::instantiate_immix(
        1024 * 1024 * 1024,
        64 * 1024 * 1024,
        128 * 1024 * 1024,
        512 * 1024 * 1024,
        true,
    );

    let stack = heap.shadow_stack();
    letroot!(x = stack, Array::from_slice(&mut heap, [1i32; 64]));
    println!("{}", x.allocation_size());
    println!("{:?}", **x);

    heap.collect(&mut []);
    let y = heap.allocate(42i32, AllocationSpace::New);
    println!("{:p}", *x);
    println!("{:p}", y);
}
