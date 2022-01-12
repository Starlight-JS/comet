use crate::bitmap::round_up;

use super::block::{Block, ATOM_SIZE, BLOCK_PAYLOAD};
/// The largest cell we're willing to allocate in a [Block] the "normal way" (i.e. using size
/// classes, rather than a large allocation) is half the size of the payload, rounded down. This
/// ensures that we only use the size class approach if it means being able to pack two things
/// into one block
pub const LARGE_CUTOFF: usize = (BLOCK_PAYLOAD / 2) & !(15);

/// We have an extra size class for size zero.
pub const NUM_SIZE_CLASSES: usize = LARGE_CUTOFF / ATOM_SIZE + 1;
/// Sizes up to this amount get a size class for each size step.
pub const PRECISE_CUTOFF: usize = 80;

pub fn size_classes(size_class_progression: f64, dump: bool) -> Vec<usize> {
    let mut result = Vec::new();
    let add = |result: &mut Vec<usize>, size| {
        let sz = round_up(size as u64, ATOM_SIZE as u64) as usize;
        result.push(sz);
    };
    let mut size = ATOM_SIZE;
    while size < PRECISE_CUTOFF {
        add(&mut result, size);
        size += ATOM_SIZE;
    }

    for i in 0usize.. {
        let approximate_size = PRECISE_CUTOFF as f64 * size_class_progression.powi(i as _);
        let approximate_size_in_bytes = approximate_size as usize;
        if approximate_size_in_bytes > LARGE_CUTOFF {
            break;
        }

        let size_class = round_up(approximate_size_in_bytes as _, ATOM_SIZE as _) as usize;

        let cells_per_block = BLOCK_PAYLOAD / size_class;
        let possibly_better_size_class = (BLOCK_PAYLOAD / cells_per_block) & !(ATOM_SIZE - 1);

        let original_wastage = BLOCK_PAYLOAD - cells_per_block * ATOM_SIZE;
        let new_wastage = (possibly_better_size_class - size_class) * cells_per_block;

        let better_size_class = if new_wastage > original_wastage {
            size_class
        } else {
            possibly_better_size_class
        };

        if Some(better_size_class) == result.last().copied() {
            continue;
        }

        if better_size_class > LARGE_CUTOFF {
            break;
        }

        add(&mut result, better_size_class);
    }

    add(&mut result, 256);

    result.sort();
    result.dedup();
    if dump {
        eprintln!("CMS Size class dump: {:?}", result);
    }
    result
}

pub fn build_size_class_table(progression: f64, dump: bool) -> [usize; NUM_SIZE_CLASSES] {
    let mut table = [0; NUM_SIZE_CLASSES];
    let mut next_index = 0;
    for size_class in size_classes(progression, dump) {
        let index = Space::size_class_to_index(size_class);
        for i in next_index..=index {
            table[i] = size_class;
        }
        next_index += 1;
    }

    for i in next_index..NUM_SIZE_CLASSES {
        table[i] = Space::index_to_size_class(i);
    }
    table
}

pub struct Space {
    size_class_for_size_step: [usize; NUM_SIZE_CLASSES],
}

impl Space {
    pub const fn size_class_to_index(size: usize) -> usize {
        (size + ATOM_SIZE - 1) / ATOM_SIZE
    }

    pub const fn index_to_size_class(index: usize) -> usize {
        let result = index * ATOM_SIZE;
        result
    }
}
