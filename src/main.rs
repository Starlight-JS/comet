use comet::{base::GcBase, letroot, minimark::MiniMarkGC};

pub struct Tree {
    first: Option<comet::api::Gc<Self>>,
    second: Option<comet::api::Gc<Self>>,
}
impl Tree {
    pub fn item_check(&self) -> i32 {
        if self.first.is_none() {
            return 1;
        }
        1 + self.first.unwrap().item_check() + self.second.unwrap().item_check()
    }
}
unsafe impl comet::api::Trace for Tree {
    fn trace(&mut self, vis: &mut dyn comet::api::Visitor) {
        self.first.trace(vis);
        self.second.trace(vis);
    }
}
unsafe impl comet::api::Finalize for Tree {}

impl comet::api::Collectable for Tree {}

pub fn bottom_up_tree(heap: &mut MiniMarkGC, mut depth: i32) -> comet::api::Gc<Tree> {
    if depth > 0 {
        depth -= 1;
        let stack = heap.shadow_stack();
        letroot!(first = stack, bottom_up_tree(heap, depth));
        letroot!(second = stack, bottom_up_tree(heap, depth));
        heap.allocate(Tree {
            first: Some(*first),
            second: Some(*second),
        })
    } else {
        heap.allocate(Tree {
            first: None,
            second: None,
        })
    }
}

pub fn bottom_up_tree_wostack(heap: &mut MiniMarkGC, mut depth: i32) -> comet::api::Gc<Tree> {
    if depth > 0 {
        depth -= 1;

        let first = bottom_up_tree_wostack(heap, depth);
        let second = bottom_up_tree_wostack(heap, depth);
        heap.allocate(Tree {
            first: Some(first),
            second: Some(second),
        })
    } else {
        heap.allocate(Tree {
            first: None,
            second: None,
        })
    }
}

fn main() {
    let mut heap = MiniMarkGC::new(None, None, None, !false);
    let min_depth = 4;
    let max_depth = 18;
    let mut depth = min_depth;
    while depth < max_depth {
        let iterations = 1 << (max_depth - depth + min_depth);

        for _ in 0..iterations {
            bottom_up_tree(&mut *heap, depth).item_check();
        }

        depth += 2;
    }
}
