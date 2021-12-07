# Comet

Comet is a garbage collector library implemented in Rust and designed to be used in language VM implementation. At the moment this library implements two garbage collector algorithms:

## Semispace 

Simple semispace GC using cheneys algorithm, nothing much to say about it. 

## MiniMark

Generational GC with two generations: nursery and old space. This GC is based on RPython's minimark but instead of using custom memory allocator for old space mimalloc is used. 

In MiniMark minor collection will copy all surviving young objects to old space and will set bump pointer to the start of nursery space again. Major collection is done
as regular mark-n-sweep without any fancy techniques. 

# Finalization

Finalization is not fully supported but running destructors is supported, although it is quite large performance hit if heap is full of objects with destructors. It is better to use `comet::alloc::*` types instead of `std::vec::Vec` or others. These types do not necessarily require destructors and are fully allocated in GC heap regions.

# Write barriers

Some GC algorithms (like MiniMark) require write barriers to manage inter generation pointers. You have to put write barrier after each write to GC object like this:

```rust
let mut heap = MiniMarkGC::new(None,None,None);
let stack = heap.shadow_stack();
letroot!(vec = stack, Vector::with_capacity(&mut *heap, 4));

let val = heap.allocate(42.42f64);

vec.push_back(&mut*heap,val);
vec.write_barrier(val,&mut*heap);
// or: 
heap.write_barrier(vec,val);

```

**NOTE**: Write barrier is not required if you write non GC allocated value to GC object. 

# Rooting

Comet is fully precise GC so it should know information about each GC pointer in the system. To do so we implement shadow stack for keeping track of roots on stack. 
To use shadow stack simply get one with calling `.shadow_stack()` on instance of `GcBase` and use it with `letroot!()` macro like this:
```rust
let mut heap = MiniMarkGC::new(None,None,None);
let stack = heap.shadow_stack();
letroot!(val = stack, heap.allocate(42.42f64)); // now `val` will be correctly traced and moved if needed.

heap.full_collection(&mut []); 
```