use std::mem::size_of;

use comet_multi::{
    alloc::array::Array,
    api::{Collectable, Finalize, Gc, Trace},
    bitmap::SpaceBitmap,
    gc_base::AllocationSpace,
    gc_vector,
    generational::{instantiate_gencon, GenConOptions},
    immix, letroot,
    utils::formatted_size,
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

struct Foo(i32);
unsafe impl Trace for Foo {
    fn trace(&mut self, _vis: &mut dyn comet_multi::api::Visitor) {
        println!("Trace Foo({})!", self.0);
    }
}

unsafe impl Finalize for Foo {}

impl Collectable for Foo {
    fn allocation_size(&self) -> usize {
        size_of::<Foo>()
    }
}

fn main() {
    println!(
        "{}",
        formatted_size(SpaceBitmap::<8>::compute_bitmap_size(1024 * 1024 * 1024))
    );
    let mut heap = immix::instantiate_immix(
        1024 * 1024 * 1024,
        64 * 1024 * 1024,
        128 * 1024 * 1024,
        512 * 1024 * 1024,
        true,
    );
    let stack = heap.shadow_stack(); // obtain &'static ShadowStack reference
    letroot!(value = stack, heap.allocate(Foo(1), AllocationSpace::New)); // allocate Foo on GC heap and put it to shadow stack.
    println!("{}", value.0); // value is Rooted<Gc<Foo>>
    heap.collect(&mut []);
}
