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
    let ms_mutator = comet_multi::marksweep::instantiate_marksweep::<false, false>(
        256 * 1024 * 1024,
        MS_DEFAULT_MAXIMUM_SIZE * 4,
        256 * 1024 * 1024,
        512 * 1024 * 1024,
        2.0,
        MS_DEFAULT_MAXIMUM_SIZE * 4,
        false,
    );

    let mut handles = vec![];
    println!("Will allocate {}", formatted_size(500_000_000 * 32));
    for _ in 0..2 {
        handles.push(ms_mutator.spawn_mutator(|mut ms_mutator| {
            let stack = ms_mutator.shadow_stack();
            letroot!(list = stack, ms_mutator.allocate(Node::None));
            let mut i = 0;
            let mut j = 0;
            while i < 500_000_000 {
                *list = ms_mutator.allocate(Node::Some {
                    value: j + 1,
                    next: *list,
                });

                j += 1;
                if i % 8192 == 0 {
                    *list = ms_mutator.allocate(Node::None);
                    j = 0;
                }
                if i % 100_000_000 == 0 {
                    println!("{:?}: Wow!", std::thread::current().id());
                }
                i += 1;
            }
            println!("{:?}: Finished", std::thread::current().id());
        }));
    }

    for handle in handles {
        handle.join(&ms_mutator);
    }
}
