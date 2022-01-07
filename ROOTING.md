# Introduction

This guide explains the basics of interacting with Comet's GC policies as a Comet API user. Since Comet implements precise GCs, it is very important that it knows about each and every pointer to a GC thing in the system. Comet's rooting API tries to make this task as simple as possible.

# What is a GC thing pointer? 

"GC thing" is the term used to refer to memory allocated and managed by the any of Comet's garbage collectors. The main types of GC thing pointer are:

- `Gc<T>`
- `Weak<T>`
- Types from `comet_extra::alloc` library are all GC thing pointers and thus must be rooted too

If you use these types directly, or create structs or arrays that contain them, you must follow the rules set out in this guide. If you do not your program will not work correctly — if it works at all.

# GC things pointers on the stack

### `Rooted<T>` ### 

All GC things pointers stored on the stack (i.e, local variables and parameters to functions) must use the `Rooted<T>` struct. This is a generic struct where the generic parameter is the type of the GC thing it contains. From the user perspective, a `Rooted<T>` instance behaves exactly as if it were the underlying pointer. 


`Rooted` must be constructed with `letroot!()` macro. 

For example instead of this: 

```rust
let heap_num = mutator.allocate(42,AllocationSpace::New);
```

You would write this: 
```rust
letroot!(heap_num = mutator.shadow_stack(), mutator.allocate(42,AllocationSpace::New));
```

# Return values

It's ok to return raw pointers! These do not need to be rooted, but they should be immediatly used to initialize a `Rooted<T>` if there is any code that could GC before the end of the containing function; a raw pointer must never be stored on the stack during a GC. 

# Performance Tweaking

If the extra overhead of exact rooting does end up adding an unacceptable cost to a specific code path, there are some tricks you can use to get better performance at the cost of more complex code: 

- Move `letroot!()` declarations above loops. Rust compiler backend sometimes is not smart enough to do LICM on `Rooted<T>`, so forward declaring a single `Rooted<T>` above the loop and re-using it on every iteration can save some cycles. 

- Raw `Gc<T>`. If you are 100% sure that there is no way for GC to happen while the pointer is on the stack, this is an option. Note: Different GCs can trigger collection because of any allocation error; GC because of concurrent GC timer; GC because we are low on memory; GC because of cosmic rays, etc. This is not a terribly safe option for embedder code, so only consider this as a very last resort. 

# GC thing pointers on the heap 

### `Gc<T>` ### 

GC thing pointers on the heap must be wrapper in a `Gc<T>`. `Gc<T>` **pointers must also continue to be traced in the normal way**, and how to do that is explained below. 

Here's how would your heap struct could look like: 

```rust
struct HeapStruct {
    some_field: Gc<i32>
}
```

# Tracing

For a regular `struct` or `enum`, tracing must be triggered manually. The way to do that is to implement `Trace` trait (you always implement it in order to allocate object on the heap) and implement `trace` method in it. Here's an example implementation:

```rust

struct Node<T: Trace> {
    next: Option<Gc<Node<T>>>,
    value: T
}

unsafe impl<T: Trace> Trace for Node<T> {
    // note that `trace` accepts `&mut self`, but it is UB to modify any of the fields
    // in there, read documentation of the `Trace` trait for more detailed information. 
    fn trace(&mut self, visitor: &mut dyn Visitor) {
        self.next.trace(visitor); // yes! Some of core/std/alloc types do implement `Trace`! 
        self.value.trace(visitor);
    }
}

// Specifies how we finalize this type. Atm finalizers are not supported. 
unsafe impl<T: Trace> Finalize for Node<T> {}

// Says to GC that we can allocate this type on the heap
impl<T: Trace> Collectable for Node<T> {}

```


### `MarkingConstraint` ### 
`MarkingConstraint` is a trait that allows you to implement your owm marking constraint! It is useful when you have some custom roots that are not rooted on stack or in any other way. Here's how simple implementation might look like: 
```rust

struct HandleList {
    handles: Vec<Gc<dyn Collectable, Immix>>
}
impl HandleList {
    pub fn add(&mut self,x: Gc<dyn Collectable,Immix>) -> usize {
        self.handles.push(x);
        self.handles.len() - 1
    } 

    pub fn get(&self,ix: usize) -> Option<&Gc<dyn Collectable,Immix>> {
        self.handles.get(ix)
    }

    pub fn get_mut(&mut self,ix: usize) -> Option<&mut Gc<dyn Collectable,Immix>> {
        self.handles.get_mut(ix)
    }
}
struct MarkHandleList(*mut HandleList);

impl MarkingConstraint for MarkHandleList {
    fn run(&mut self,vis: &mut dyn Visitor) {
        unsafe {
            for handle in (*self.0).iter_mut() {
                handle.trace(handle);
            }
        }
    }

    fn is_over(&self) -> bool {
        false // never stop tracing handle lists
    }

    fn runs_at(&self) -> MarkingConstraintRuns {
        MarkingConstraintRuns::BeforeMark
    }
    
    fn name(&self) -> &str {
        "MarkHandleList"
    }
}

fn main() {
    let mut handle_list = Box::leak(Box::new(HandleList {
        handles: vec![]
    }));

    let constraint = MarkHandleList(handle_list as *mut _); // yes, very unsafe. You can do safe code in your implementation tho

    let mut mutator = comet::immix::instantiate_immix(...);

    mutator.add_constraint(constraint);
    let val = mutator.allocate(42,AllocationSpace::New);
    let val = handle_list.add(val);
    mutator.collect(&mut []);
    println!("{}",*handle_list.get(val.to_dyn()).unwrap());
}

```


# Summary

- Use `Rooted<T>` and `letroot!()` for local variables on the stack.
- Return raw `Gc<T>` pointers from functions.
- Use `Gc<T>` or `Weak<T>` members for heap data. Note: they are not "rooted": they must be traced!
- Do not use `Rooted<T>` for function parameters 
- Use `MarkingConstraint` for things that are alive for a long period of time and cannot be rooted using `letroot!()`.
