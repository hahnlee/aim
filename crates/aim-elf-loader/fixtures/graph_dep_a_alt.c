static int state;

__attribute__((constructor)) static void initialize_dep_a(void) {
  state = 100;
}

int dep_a_value(void) {
  return state;
}
