extern void guest_libdl_record(int phase);

__attribute__((constructor)) static void GuestPluginInit(void) {
  guest_libdl_record(1);
}

__attribute__((destructor)) static void GuestPluginFini(void) {
  guest_libdl_record(4);
}

__attribute__((visibility("default"))) int guest_plugin_value(int input) {
  return input + 7;
}
