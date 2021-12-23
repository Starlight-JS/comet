use comet_multi::{
    api::{Collectable, Finalize, Gc, Trace},
    letroot,
    marksweep::*,
    mutator::MutatorRef,
    utils::formatted_size,
};

pub enum Node {
    None,
    Some { value: i64, next: Gc<Node> },
}

unsafe impl Trace for Node {
    fn trace(&mut self, vis: &mut dyn comet_multi::api::Visitor) {
        match self {
            Self::Some { next, value } => {
                next.trace(vis);
            }
            _ => (),
        }
    }
}

unsafe impl Finalize for Node {}
impl Collectable for Node {}

fn main() {
    // 1GB Mark&Sweep space. First GC will happen at 256MB memory usage
    let mut ms_mutator = comet_multi::marksweep::instantiate_marksweep::<false, false>(
        1 * 1024 * 1024,
        16 * 1024 * 1024,
        1 * 1024 * 1024,
        8 * 1024 * 1024,
        2.0,
        32 * 1024 * 1024,
        false,
    );

    let stack = ms_mutator.shadow_stack();
    letroot!(list = stack, ms_mutator.allocate(Node::None));
    let mut i = 0;
    let mut j = 0;
    let x = std::time::Instant::now();
    while i < 50_000_000 {
        *list = ms_mutator.allocate(Node::Some {
            value: j + 1,
            next: *list,
        });

        j += 1;
        if i % 30000 == 0 {
            *list = ms_mutator.allocate(Node::None);
            j = 0;
        }

        i += 1;
    }

    println!("{}", x.elapsed().as_millis());
}
