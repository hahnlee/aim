extern int cycle_b_value(void);

int cycle_a_value(void) {
  return cycle_b_value() + 1;
}
