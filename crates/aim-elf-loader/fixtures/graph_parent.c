extern int dep_a_value(void);
extern int dep_b_value(void);
extern int optional_graph_value(void) __attribute__((weak));
static int state;

__attribute__((constructor)) static void initialize_parent(void) {
  state = dep_a_value() + dep_b_value();
  if (optional_graph_value != 0) {
    state += optional_graph_value();
  }
}

int graph_value(void) {
  return state;
}
