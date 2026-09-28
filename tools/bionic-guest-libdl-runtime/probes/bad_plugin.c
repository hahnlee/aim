extern int guest_libdl_missing(void);

__attribute__((visibility("default"))) int guest_bad_plugin_value(void) {
  return guest_libdl_missing();
}
