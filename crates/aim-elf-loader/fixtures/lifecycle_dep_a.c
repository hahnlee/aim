extern void lifecycle_record(int);

__attribute__((destructor(101))) static void dep_a_fini_first(void) {
    lifecycle_record(101);
}

__attribute__((destructor(102))) static void dep_a_fini_second(void) {
    lifecycle_record(102);
}

void dep_a_dt_fini(void) {
    lifecycle_record(109);
}

int lifecycle_dep_a_value(void) {
    return 10;
}
