use comet::{heap::Heap, internal::gc_info::GCInfoTrait, Config, GCPlatform};

fn main() {
    GCPlatform::initialize();
    let mut config = Config::default();
    config.verbose = true;

    let mut heap = Heap::new(config);
    heap.add_core_constraints();
    unsafe {
        let mem = heap.allocate_raw(48, u32::index()).unwrap();
        heap.collect_garbage();
        let mem2 = heap.allocate_raw(48, u32::index()).unwrap();
        println!("{:p} {:p} {:p}", &mem, mem, mem2);
    }
}
