#include <stdarg.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdbool.h>
/// At maximum [`MAX_INDEX - 1`](GCInfoTable::MAX_INDEX) indices are supported.
///
/// We assume that 14 bits are enough to represent all possible types.
static const uint16_t GCInfoTable_MAX_INDEX = (1 << 14);

/// Minimum index returned. Values smaller [`MIN_INDEX`](GCInfoTable::MIN_INDEX) may be used as
/// sentinels.
static const uint16_t GCInfoTable_MIN_INDEX = 1;

static const uint16_t GCInfoTable_INITIAL_WANTED_LIMIT = 512;

static const uintptr_t IMMIX_BLOCK_SIZE = (32 * 1024);

static const uintptr_t LINE_SIZE = 256;

static const uintptr_t LINE_COUNT = (IMMIX_BLOCK_SIZE / LINE_SIZE);

static const uintptr_t LARGE_CUTOFF = (IMMIX_BLOCK_SIZE / 4);

/// Objects larger than medium cutoff span multiple lines and require special Overflow allocator
static const uintptr_t MEDIUM_CUTOFF = LINE_SIZE;

static const uintptr_t BLOCK_SIZE = (16 * 1024);

static const uintptr_t CardTable_CARD_SHIFT = 10;

static const uint8_t CardTable_CARD_CLEAN = 0;

static const uint8_t CardTable_CARD_DIRTY = 112;

typedef uint16_t EncodedHigh;
typedef struct Heap
{
} Heap;
typedef struct Visitor
{
} Visitor;

typedef struct HeapObjectHeader
{
    uint32_t _padding;
    EncodedHigh encoded_high;
    uint16_t encoded_low;
} HeapObjectHeader;

typedef struct Config
{
    double heap_growth_factor;
    double heap_growth_threshold;
    double large_heap_growth_factor;
    double large_heap_growth_threshold;
    bool dump_size_classes;
    double size_class_progression;
    uintptr_t heap_size;
    uintptr_t max_heap_size;
    uintptr_t max_eden_size;
    bool verbose;
    bool generational;
} Config;

typedef struct UntypedGcRef
{
    HeapObjectHeader *header;
} UntypedGcRef;

typedef struct WeakGcRef
{
    HeapObjectHeader *slot;
} WeakGcRef;

typedef uint16_t GCInfoIndex;

typedef void (*FinalizationCallback)(uint8_t *);

typedef void (*TraceCallback)(Visitor *, const uint8_t *);

/// GCInfo contains metadata for objects.
typedef struct GCInfo
{
    FinalizationCallback finalize;
    TraceCallback trace;
    uintptr_t vtable;
} GCInfo;

uintptr_t comet_gc_size(const HeapObjectHeader *ptr);

Config comet_default_config();

void comet_init();

Heap *comet_heap_create(Config config);

/// Free comet heap
void comet_heap_free(Heap *heap);

/// Add GC constraint to the Comet Heap. Each constraint is executed when marking starts
/// to obtain list of root objects.
void comet_heap_add_constraint(Heap *heap, uint8_t *data, void (*callback)(uint8_t *, Visitor *));

/// Add core constraints to the heap. This one will setup stack scanning routines.
void comet_heap_add_core_constraints(Heap *heap);

void comet_heap_collect(Heap *heap);

void comet_heap_collect_if_necessary_or_defer(Heap *heap);

WeakGcRef comet_heap_allocate_weak(Heap *heap, HeapObjectHeader *object);

/// Allocates memory and returns pointer. NULL is returned if no memory is available.
HeapObjectHeader *comet_heap_allocate(Heap *heap, uintptr_t size, GCInfoIndex index);

/// Allocates memory and returns pointer. When no memory is left process is aborted.
HeapObjectHeader *comet_heap_allocate_or_fail(Heap *heap, uintptr_t size, GCInfoIndex index);

/// Upgrade weak ref. If it is still alive then pointer is returned otherwise NULL is returned.
HeapObjectHeader *comet_weak_upgrade(WeakGcRef weak);

void comet_trace(Visitor *vis, HeapObjectHeader *ptr);

void comet_trace_conservatively(Visitor *vis, const uint8_t *from, const uint8_t *to);

GCInfoIndex comet_add_gc_info(GCInfo info);

GCInfo *comet_get_gc_info(GCInfoIndex index);
