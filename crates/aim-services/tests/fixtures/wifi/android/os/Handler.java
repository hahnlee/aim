package android.os;
import java.util.ArrayDeque;
public class Handler {
    private final ArrayDeque<Runnable> work = new ArrayDeque<>();
    public boolean post(Runnable task) { work.add(task); return true; }
    public void drain() { while (!work.isEmpty()) work.remove().run(); }
}
