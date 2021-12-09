use comet::{
    api::{Collectable, Finalize, Gc, Trace},
    base::GcBase,
    letroot,
    minimark::MiniMarkGC,
    util::formatted_size,
};

#[repr(C)]
pub struct Node {
    left: Option<Gc<Node>>,
    right: Option<Gc<Node>>,
}

unsafe impl Trace for Node {
    fn trace(&mut self, _vis: &mut dyn comet::api::Visitor) {
        self.left.trace(_vis);
        self.right.trace(_vis);
    }
}

unsafe impl Finalize for Node {}
impl Collectable for Node {}

#[inline]
fn bottom_up_tree(heap: &mut MiniMarkGC, depth: i32, allocated: &mut usize) -> Gc<Node> {
    if depth <= 0 {
        *allocated += 1;
        return heap.allocate(Node {
            right: None,
            left: None,
        });
    }
    let stack = heap.shadow_stack();
    letroot!(left = stack, bottom_up_tree(heap, depth - 1, allocated));
    letroot!(right = stack, bottom_up_tree(heap, depth - 1, allocated));
    *allocated += 1;
    heap.allocate(Node {
        left: Some(*left),
        right: Some(*right),
    })
}

impl Node {
    pub fn item_check(&self) -> i32 {
        if self.left.is_none() {
            return 1;
        }
        1 + self.left.unwrap().item_check() + self.right.unwrap().item_check()
    }
}
enum List {
    None,
    Some { value: i64, next: Gc<Self> },
}

unsafe impl Trace for List {
    fn trace(&mut self, _vis: &mut dyn comet::api::Visitor) {
        match self {
            Self::Some { next, .. } => next.trace(_vis),
            _ => (),
        }
    }
}
unsafe impl Finalize for List {}

impl Collectable for List {}
fn main() {
    let mut heap = MiniMarkGC::new(Some(64 * 1024 * 1024), None, None, !true);
    let stretch_depth = 19;

    let stack = heap.shadow_stack();
    /*let mut n = 0;
    letroot!(
        tree = stack,
        bottom_up_tree(&mut *heap, stretch_depth, &mut n)
    );
    /*for depth in 6..16 {
        let iterations = 1 << (16 - depth + 4);
        for _ in 0..iterations {
            bottom_up_tree(&mut *heap, depth, &mut n);
        }
        println!("{} trees of depth {}", iterations, depth);
    }*/
    //heap.full_collection(&mut []);
    println!("{:p} {:p}", *tree, &tree);
    println!("approximate heap size: {}", formatted_size(n * 32));*/
    letroot!(list = stack, heap.allocate(List::None));
    let start = std::time::Instant::now();
    let mut i = 0;
    while i < 500_000_000 {
        *list = heap.allocate(List::Some {
            value: 42,
            next: *list,
        });
        if i % 8192 == 0 {
            *list = heap.allocate(List::None);
        }
        i += 1;
    }
    println!("{}", start.elapsed().as_millis());
}
