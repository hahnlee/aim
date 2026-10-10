// Compile-only pinned original API.
package com.android.server;
public final class FgThread extends ServiceThread {
 private FgThread(){super("unused compile stub",0,false);throw new RuntimeException("stub");} public static java.util.concurrent.Executor getExecutor() { throw new RuntimeException("stub"); }  public static android.os.Handler getHandler(){throw new RuntimeException("stub");}
}
