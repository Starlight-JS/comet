#include "c_api.h"
#include <stdio.h>
typedef struct Node
{
    HeapObjectHeader *hdr;
    struct Node *next;
    int val;
} Node;

void node_trace(Visitor *vis, const uint8_t *ptr)
{
    printf("trace Node %p with val %i\n", ptr, ((Node *)ptr)->val);
    comet_trace(vis, (HeapObjectHeader *)((Node *)ptr)->next);
}

void node_finalize(uint8_t *ptr)
{
    Node *node = (Node *)ptr;
    printf("Finalize node at %p with val %i\n", node, node->val);
}
void foo(Heap *heap)
{
    GCInfoIndex index = comet_add_gc_info((GCInfo){node_finalize, node_trace, 0});
    Node *node = comet_heap_allocate_or_fail(heap, sizeof(Node), index);
    node->next = NULL;
    node->val = 0;
    comet_heap_collect(heap);
    printf("%p\n", &node);
    node = NULL;
}
int main()
{
    comet_init();
    Config c = comet_default_config();
    c.verbose = true;
    Heap *heap = comet_heap_create(c);
    comet_heap_add_core_constraints(heap);
    foo(heap);
    comet_heap_collect(heap);
    comet_heap_free(heap);
}