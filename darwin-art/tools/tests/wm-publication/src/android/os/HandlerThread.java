package android.os;

import java.util.ArrayDeque;

/** Minimal manually-driven HandlerThread replacement for the WMS fixture. */
public final class HandlerThread {
    private static HandlerThread latest;
    private final ArrayDeque<Runnable> queue = new ArrayDeque<>();
    private final Looper looper = new Looper(this);
    private boolean failPosts;
    private boolean throwPosts;
    private final String name;

    public HandlerThread(String name) { this.name = name; }

    public void start() {
        if ("WindowManagerInputPublication".equals(name)) latest = this;
    }

    public Looper getLooper() { return looper; }

    boolean post(Runnable task) {
        if (throwPosts) throw new AssertionError("fixture Handler post failure");
        if (failPosts) return false;
        queue.addLast(task);
        return true;
    }

    public void setFailPosts(boolean value) { failPosts = value; }

    public void setThrowPosts(boolean value) { throwPosts = value; }

    /** This transport-only fixture selects the latest WMS publication owner, not callback delivery. */
    public static HandlerThread latest() { return latest; }

    /** Runs one queued task, returning false when this thread is idle. */
    public boolean runNext() {
        Runnable task = queue.pollFirst();
        if (task == null) return false;
        task.run();
        return true;
    }

    public static boolean runLatestNext() {
        return latest != null && latest.runNext();
    }
}
