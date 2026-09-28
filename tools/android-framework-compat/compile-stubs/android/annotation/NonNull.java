package android.annotation;

import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;

/** Compile-only declaration; Android's framework DEX owns the runtime class. */
@Retention(RetentionPolicy.CLASS)
public @interface NonNull {}
