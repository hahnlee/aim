extern void lifecycle_record(int);
extern int lifecycle_dep_a_value(void);

__attribute__((destructor(101))) static void dep_b_fini_first(void) {
    lifecycle_record(201);
}

__attribute__((destructor(102))) static void dep_b_fini_second(void) {
    lifecycle_record(202);
}

void dep_b_dt_fini(void) {
    lifecycle_record(209);
}

int lifecycle_dep_b_value(void) {
    return lifecycle_dep_a_value() + 10;
}
