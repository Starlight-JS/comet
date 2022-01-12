use comet::cms::space::build_size_class_table;

fn main() {
    let table = build_size_class_table(1.34, true);
    println!("{:?}", table);
}
