use std::mem::size_of;

use crate::{block::Block, internal::BLOCK_SIZE};

/// Sizes up to this amount get a size class for each size step.
const PRECISE_CUTOFF: usize = 80;
const SIZE_STEP: usize = 16;
pub const fn round_up(x: usize, y: usize) -> usize {
    ((x) + (y - 1)) & !(y - 1)
}
const LARGE_CUTOFF: usize = ((BLOCK_SIZE - size_of::<Block>()) / 2) & !(SIZE_STEP - 1);
const BLOCK_PAYLOAD: usize = BLOCK_SIZE - size_of::<Block>();
fn generate_size_classes(dump_size_classes: bool, sz_class_progression: f64) -> Vec<usize> {
    let mut result = vec![];
    let mut add = |result: &mut Vec<usize>, size_class| {
        logln_if!(dump_size_classes, "Adding size class: {}", size_class);
        if result.is_empty() {
            assert_eq!(size_class, 16);
        }
        result.push(size_class);
    };

    let mut size = 16;
    while size < PRECISE_CUTOFF {
        add(&mut result, size);
        size += SIZE_STEP;
    }
    logln_if!(
        dump_size_classes,
        "       Block payload size: {}",
        BLOCK_SIZE - offsetof!(Block.data_start)
    );

    for i in 0.. {
        let approximate_size = PRECISE_CUTOFF as f64 * sz_class_progression.powi(i);
        logln_if!(
            dump_size_classes,
            "     Next size class as a double: {}",
            approximate_size
        );
        let approximate_size_in_bytes = approximate_size as usize;
        logln_if!(
            dump_size_classes,
            "     Next size class as bytes: {}",
            approximate_size_in_bytes
        );
        assert!(approximate_size_in_bytes >= PRECISE_CUTOFF);

        if approximate_size_in_bytes >= LARGE_CUTOFF {
            break;
        }
        let size_class = round_up(approximate_size_in_bytes, SIZE_STEP);
        logln_if!(dump_size_classes, "     Size class: {}", size_class);

        let cells_per_block = BLOCK_PAYLOAD / size_class;
        let possibly_better_size_class = (BLOCK_PAYLOAD / cells_per_block) & !(SIZE_STEP - 1);
        logln_if!(
            dump_size_classes,
            "     Possibly better size class: {}",
            possibly_better_size_class
        );
        let original_wastage = BLOCK_PAYLOAD - cells_per_block * size_class;
        let new_wastage = (possibly_better_size_class - size_class) * cells_per_block;
        logln_if!(
            dump_size_classes,
            "    Original wastage: {}, new wastage: {}",
            original_wastage,
            new_wastage
        );

        let better_size_class = if new_wastage > original_wastage {
            size_class
        } else {
            possibly_better_size_class
        };
        logln_if!(
            dump_size_classes,
            "    Choosing size class: {}",
            better_size_class
        );
        if Some(better_size_class) == result.last().copied() {
            // when size class step is too small
            continue;
        }

        if better_size_class > LARGE_CUTOFF {
            break;
        }
        add(&mut result, better_size_class);
    }
    // Manually inject size classes for objects we know will be allocated in high volume.

    add(&mut result, 256);
    //add(&mut result, size_of::<JsObject>());
    result.sort_unstable();
    result.dedup();
    result.shrink_to_fit();
    logln_if!(dump_size_classes, "Heap size class dump: {:?}", result);

    result
}

pub const NUM_SIZE_CLASSES: usize = LARGE_CUTOFF / SIZE_STEP + 1;
fn build_size_class_table(
    dump: bool,
    progression: f64,
    table: &mut [usize],
    cons: impl Fn(usize) -> usize,
    default_cons: impl Fn(usize) -> usize,
) {
    let mut next_index = 0;
    for sz in generate_size_classes(dump, progression) {
        let entry = cons(sz);
        let index = size_class_to_index(sz);
        for i in next_index..=index {
            table[i] = entry;
        }
        next_index = index + 1;
    }
    for i in next_index..NUM_SIZE_CLASSES {
        table[i] = default_cons(index_to_size_class(i));
    }
}
fn initialize_size_class_for_step_size(dump: bool, progression: f64, table: &mut [usize]) {
    build_size_class_table(dump, progression, table, |sz| sz, |sz| sz);
}

const fn size_class_to_index(size: usize) -> usize {
    (size + SIZE_STEP - 1) / SIZE_STEP
}

fn index_to_size_class(index: usize) -> usize {
    let result = index * SIZE_STEP;
    debug_assert_eq!(size_class_to_index(result), index);
    result
}

pub struct GlobalHeap {}
