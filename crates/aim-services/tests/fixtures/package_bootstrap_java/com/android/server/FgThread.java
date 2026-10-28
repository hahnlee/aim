package com.android.server;
public final class FgThread {
    public static final java.util.ArrayDeque<Runnable> pending = new java.util.ArrayDeque<>();
    public static java.util.concurrent.Executor getExecutor() { return pending::add; }
}
