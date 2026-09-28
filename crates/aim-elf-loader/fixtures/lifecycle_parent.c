extern void lifecycle_record(int);
extern int lifecycle_dep_b_value(void);

__attribute__((destructor(101))) static void parent_fini_first(void) {
    lifecycle_record(301);
}

__attribute__((destructor(102))) static void parent_fini_second(void) {
    lifecycle_record(302);
}

void parent_dt_fini(void) {
    lifecycle_record(309);
}

int lifecycle_graph_value(void) {
    return lifecycle_dep_b_value() + 10;
}
