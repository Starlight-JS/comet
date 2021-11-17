use comet_api::*;

enum Cell {
    Some { next: GcPointer<Cell>, value: i64 },
    None,
}

impl Trace for Cell {
    fn trace(&self, vis: &mut Visitor) {
        match self {
            Self::Some { next, .. } => next.trace(vis),
            Self::None => {}
        }
    }
}

impl Finalize<Self> for Cell {}
impl GcCell for Cell {}
fn main() {
    GCPlatform::initialize();
    let mut heap = Heap::new(false, Some(1024 * 1024 * 1024));
    let start = std::time::Instant::now();
    let mut i = 0;
    let mut l = heap.allocate(Cell::None);
    println!("{}", std::mem::size_of::<Cell>());
    while i < 5_000_000 {
        l = heap.allocate(Cell::Some { next: l, value: 42 });
        if i % 100000 == 0 {
            l = heap.allocate(Cell::None);
        }
        heap.collect_if_necessary();
        i += 1;
    }
    let time_in_secs = start.elapsed().as_millis() as f64 / 1000.0;

    println!("ran in {} secs", time_in_secs);

    let stats = heap.statistics();
    println!(
        "{}",
        stats.total_memory_allocated as f64 / 1024.0 / 1024.0 / 1024.0
    );
    println!(
        "throughput: {:.2} GB/S",
        (stats.total_memory_allocated as f64 / 1024.0 / 1024.0 / 1024.0) / time_in_secs
    );
    println!("{}", stats);
}
