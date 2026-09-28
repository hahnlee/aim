extern int dep_a_value(void);
static int state;

__attribute__((constructor)) static void initialize_dep_b(void) {
  state = dep_a_value() + 2;
}

__attribute__((visibility("protected"))) int dep_b_value(void) {
  return state;
}
