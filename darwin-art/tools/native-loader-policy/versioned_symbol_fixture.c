int old_value(void) { return 11; }
int new_value(void) { return 22; }
int global_value(void) { return 33; }
__asm__(".symver old_value,versioned_value@TEST_1");
__asm__(".symver new_value,versioned_value@@TEST_2");
