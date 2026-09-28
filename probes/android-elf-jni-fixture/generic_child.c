extern int AimGenericGrandchildValue(void);

__attribute__((visibility("default"))) int AimGenericChildValue(void) {
  return AimGenericGrandchildValue() + 10;
}
