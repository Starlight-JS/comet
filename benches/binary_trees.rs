use comet::minimark::MiniMarkGC;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

pub fn bench_gcs(c: &mut Criterion) {
    let mut group = c.benchmark_group("binary trees");
    group.sample_size(50);
    let n = 12;

    for i in n..19 {
        let min_depth = 4;
        let mut max_depth = min_depth + 2;
        if max_depth < n {
            max_depth = n;
        }
        group.bench_function(BenchmarkId::new("minimark", i), |b| {
            b.iter_batched_ref(
                || MiniMarkGC::new(None, None, None, false),
                |heap| {
                    let mut depth = min_depth;
                    while depth < max_depth {
                        let iterations = 1 << (max_depth - depth + min_depth);

                        for _ in 0..iterations {
                            comet_tree::bottom_up_tree(&mut **heap, depth).item_check();
                        }

                        depth += 2;
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });

        group.bench_function(BenchmarkId::new("minimark(conservative)", i), |b| {
            b.iter_batched_ref(
                || MiniMarkGC::new(None, None, None, true),
                |heap| {
                    let mut depth = min_depth;
                    while depth < max_depth {
                        let iterations = 1 << (max_depth - depth + min_depth);
                        for _ in 0..iterations {
                            comet_tree::bottom_up_tree_wostack(&mut **heap, depth);
                        }
                        depth += 2;
                    }
                },
                criterion::BatchSize::LargeInput,
            );
        });

        group.bench_function(BenchmarkId::new("rust-gc", i), |b| {
            b.iter_batched(
                || (),
                |_: ()| {
                    let mut depth = min_depth;
                    while depth < max_depth {
                        let iterations = 1 << (max_depth - depth + min_depth);

                        for _ in 0..iterations {
                            rust_gc_tree::bottom_up_tree(depth).item_check();
                        }

                        depth += 2;
                    }
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

criterion_group!(benches, bench_gcs);
criterion_main!(benches);

mod comet_tree {
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
}

mod rust_gc_tree {

    use gc::*;
    use gc_derive::*;

    #[derive(Trace, Finalize)]
    pub struct Tree {
        first: Option<Gc<Self>>,
        second: Option<Gc<Self>>,
    }

    pub fn bottom_up_tree(mut depth: i32) -> Gc<Tree> {
        if depth > 0 {
            depth -= 1;

            let first = bottom_up_tree(depth);
            let second = bottom_up_tree(depth);
            Gc::new(Tree {
                first: Some(first),
                second: Some(second),
            })
        } else {
            Gc::new(Tree {
                second: None,
                first: None,
            })
        }
    }

    impl Tree {
        pub fn item_check(&self) -> i32 {
            if self.first.is_none() {
                return 1;
            }
            1 + self.first.as_ref().unwrap().item_check()
                + self.second.as_ref().unwrap().item_check()
        }
    }
}
