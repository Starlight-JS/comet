use comet::{gc_base::AllocationSpace, letroot, semispace::instantiate_semispace};

fn main() {
    let mut mutator = instantiate_semispace(64 * 1024);
    let stack = mutator.shadow_stack();
    let my_obj = mutator.allocate(42i32, AllocationSpace::New); // note that this object is unprotected and it will be recycled during GC cycle
    letroot!(
        my_obj2 = stack,
        mutator.allocate(44i32, AllocationSpace::New)
    ); // my_obj2 is protected so it will survive next GC cycle
    letroot!(my_weak = stack, mutator.allocate_weak(*my_obj2)); // allocate weak reference for my_obj2
    letroot!(my_weak2 = stack, mutator.allocate_weak(my_obj)); // allocate weak reference for my_obj

    println!("my_weak holds: {}", *my_weak.upgrade().unwrap());
    println!("my_weak2 holds: {}", *my_weak2.upgrade().unwrap());
    mutator.collect(&mut []);

    println!("my_weak2 is empty: {}", my_weak2.upgrade().is_none());
    println!("my_weak holds: {}", *my_weak.upgrade().unwrap());
}
