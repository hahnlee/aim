// Compile-only pinned image API; never packaged as runtime implementation.
package com.android.internal.util;
public class FrameworkStatsLog {
    public FrameworkStatsLog() { throw new RuntimeException("stub"); }
 public static final int APP_DOWNGRADED=128;
 public static void write(int code,String name,long before,long after,boolean aggressive){throw new RuntimeException("stub");}
}
