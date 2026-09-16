#if defined(IMPORT_PARENT)
extern int parent_value;
#elif defined(PROTECTED_VALUE)
__attribute__((visibility("protected"))) int parent_value = 11;
#else
int parent_value = 11;
#endif
int child_value(void) { return parent_value + 5; }
