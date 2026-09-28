package android.compat.annotation;

// Signature-only javac view used by pinned hidden-platform sources. This
// annotation and every class compiled alongside it stay outside application DEX.
public @interface UnsupportedAppUsage {
  int maxTargetSdk() default Integer.MAX_VALUE;
  long trackingBug() default 0L;
}
