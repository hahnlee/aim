extern void lifecycle_record(int);

__attribute__((destructor(101))) static void fini_array_first(void) {
    lifecycle_record(11);
}

__attribute__((destructor(102))) static void fini_array_second(void) {
    lifecycle_record(12);
}

void finalizer_dt_fini(void) {
    lifecycle_record(19);
}

__attribute__((visibility("default"))) int finalizer_value(void) {
    return 42;
}
