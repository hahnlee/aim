extern int cycle_a_value(void);
static int (*volatile peer)(void) = cycle_a_value;

int cycle_b_value(void) {
  return 20 + (peer != 0);
}
