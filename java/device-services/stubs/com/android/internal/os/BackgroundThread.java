// Compile-only pinned image API.
package com.android.internal.os;
public final class BackgroundThread extends android.os.HandlerThread {
 private BackgroundThread(){super("unused compile stub");throw new RuntimeException("stub");}
    public static android.os.Handler getHandler() { throw new RuntimeException("stub"); }
 public static java.util.concurrent.Executor getExecutor(){throw new RuntimeException("stub");}
}
