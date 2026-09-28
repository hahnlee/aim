static int selected_implementation(void) {
  return 1032;
}

static void* resolve_selected(void) {
  return (void*)selected_implementation;
}

static int selected(void)
    __attribute__((ifunc("resolve_selected"), visibility("hidden")));

int fixture_ifunc_value(void) {
  return selected();
}
