// Host-only Android boundary double; production policy is compiled unchanged.
package android.os;
public class Handler {
    public static final java.util.Queue<Runnable> tasks = new java.util.ArrayDeque<>();
    public Handler(Looper looper) {}
    public boolean post(Runnable runnable) { tasks.add(runnable); return true; }
    public static void drain() { while(!tasks.isEmpty()) tasks.remove().run(); }
}
