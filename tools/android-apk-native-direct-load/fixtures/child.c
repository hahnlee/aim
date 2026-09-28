extern int AimApkDirectGrandchildValue(void);

__attribute__((visibility("default"))) int AimApkDirectChildValue(void) {
  return AimApkDirectGrandchildValue() + 20;
}
