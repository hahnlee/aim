extern void record_finalizer(int);
#if ORDER_NODE == 0
extern int node1(void);
extern int node2(void);
int node0(void) { return node1() + node2(); }
#elif ORDER_NODE == 1
extern int node3(void);
int node1(void) { return node3(); }
#elif ORDER_NODE == 2
extern int node3(void);
int node2(void) { return node3(); }
#else
int node3(void) { return 3; }
#endif
__attribute__((destructor)) static void finish(void) { record_finalizer(ORDER_NODE); }
