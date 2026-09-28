extern int AimApkDirectChildValue(void);

static int graph_value;

__attribute__((constructor)) static void Initialize(void) {
  graph_value = AimApkDirectChildValue();
}

__attribute__((visibility("default"))) int JNI_OnLoad(void* vm, void* reserved) {
  return vm != 0 && reserved == 0 && graph_value == 42 ? 0x00010006 : -1;
}
