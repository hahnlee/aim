static volatile int value;
#ifdef OBSERVE_LINKED_IMAGE
extern int observe_linked_image(void);
__attribute__((constructor)) static void initialize(void) { value += observe_linked_image(); }
#else
__attribute__((constructor)) static void initialize(void) { value += 42; }
#endif
int initialized_value(void) { return value; }
static volatile int finalizer_calls;
__attribute__((destructor)) static void finalize(void) { ++finalizer_calls; }
int finalized_count(void) { return finalizer_calls; }
