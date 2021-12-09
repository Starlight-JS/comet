use comet::{
    api::{Collectable, Finalize, Gc, Trace},
    base::GcBase,
    letroot,
    minimark::MiniMarkGC,
};

struct LargeBox {
    x: Option<Gc<i32>>,
}

impl Collectable for LargeBox {
    fn allocation_size(&self) -> usize {
        64 * 1024
    }
}

unsafe impl Trace for LargeBox {
    fn trace(&mut self, _vis: &mut dyn comet::api::Visitor) {
        self.x.trace(_vis);
    }
}

unsafe impl Finalize for LargeBox {}

fn main() {
    let mut heap = MiniMarkGC::new(None, None, None, true);
    let shadow_stack = heap.shadow_stack();
    letroot!(x = shadow_stack, heap.allocate(LargeBox { x: None }));
    letroot!(y = shadow_stack, Some(heap.allocate(42)));
    x.x = *y;

    heap.minor_collection(&mut []);

    *y = None;
    //heap.full_collection(&mut []);
    heap.minor_collection(&mut []);
}
