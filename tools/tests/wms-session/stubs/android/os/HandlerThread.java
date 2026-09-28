package android.os;

import java.util.ArrayDeque;
import java.util.ArrayList;

/** Manually advanced delayed-handler fixture for asynchronous WMS publication. */
public final class HandlerThread {
    private static HandlerThread latest;
    private static final ArrayList<HandlerThread> ALL = new ArrayList<>();
    private final ArrayDeque<Runnable> queue = new ArrayDeque<>();
    private final Looper looper = new Looper(this);

    public HandlerThread(String name) {}
    public synchronized void start() {
        latest = this;
        synchronized (HandlerThread.class) { ALL.add(this); }
    }
    public Looper getLooper() { return looper; }
    boolean post(Runnable task) { queue.addLast(task); return true; }
    public static boolean runLatestNext() {
        if (latest != null && latest.runNext()) return true;
        synchronized (HandlerThread.class) {
            for (int i = ALL.size() - 1; i >= 0; --i) {
                if (ALL.get(i).runNext()) return true;
            }
        }
        return false;
    }
    /** Runs one queued task from any fixture HandlerThread, newest first. */
    public static boolean runAnyNext() {
        synchronized (HandlerThread.class) {
            for (int i = ALL.size() - 1; i >= 0; --i) {
                if (ALL.get(i).runNext()) return true;
            }
        }
        return false;
    }
    /** Clears deterministic fixture queues between independently-owned WMS graphs. */
    public static void resetQueues() {
        synchronized (HandlerThread.class) {
            for (HandlerThread thread : ALL) thread.queue.clear();
            ALL.clear();
            latest = null;
        }
    }
    private boolean runNext() {
        Runnable task = queue.pollFirst();
        if (task == null) return false;
        task.run();
        return true;
    }
}
