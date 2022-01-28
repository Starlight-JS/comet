use comet::{
    api::{Collectable, Finalize, Trace},
    immix::{instantiate_immix, ImmixOptions},
};
struct Foo {
    x: i32,

    z: i32,
}

unsafe impl Trace for Foo {}
unsafe impl Finalize for Foo {}
impl Collectable for Foo {}
fn main() {
    let opts = ImmixOptions::default().with_verbose(2);
    let mut immix = instantiate_immix(opts);

    let x = &immix
        .allocate(Foo { x: 0, z: 0 }, comet::gc_base::AllocationSpace::New)
        .z;
    let y = immix.allocate(2, comet::gc_base::AllocationSpace::New);
    immix.collect(&mut []);
    println!("{:p} {:p}", &x, y);
}
