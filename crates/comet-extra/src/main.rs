use comet_extra::alloc::hash::HashMap;

fn main() {
    let mut heap = comet::immix::instantiate_immix(
        128 * 1024 * 1024,
        32 * 1024 * 1024,
        4 * 1024 * 1024,
        128 * 1024 * 1024,
        true,
    );

    let mut map = HashMap::new(&mut heap);
    map.insert(&mut heap, 0i32, 1i32);
    map.insert(&mut heap, 1, 2);
    map.insert(&mut heap, 2, 3);
    println!("{}", map.get(&2).unwrap());
    for (_, val) in map.iter_mut() {
        *val += 42;
    }
    for (key, val) in map.iter() {
        println!("{}->{}", key, val);
    }
}
