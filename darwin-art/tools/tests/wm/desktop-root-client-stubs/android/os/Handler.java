package android.os;

import java.util.ArrayList;
import java.util.List;

public class Handler {
    private static final class Entry {
        final Runnable runnable; final Object token;
        Entry(Runnable value, Object key) { runnable = value; token = key; }
    }
    private final List<Entry> queue = new ArrayList<Entry>();
    public static boolean rejectPosts;
    public Handler(Looper looper) {
        if (looper == null) throw new IllegalStateException("no looper");
    }
    public boolean postAtTime(Runnable runnable, Object token, long when) {
        if (rejectPosts) return false;
        queue.add(new Entry(runnable, token));
        return true;
    }
    public void removeCallbacksAndMessages(Object token) {
        for (int i = queue.size() - 1; i >= 0; --i)
            if (queue.get(i).token == token) queue.remove(i);
    }
    public void runAll() {
        while (!queue.isEmpty()) queue.remove(0).runnable.run();
    }
    public int size() { return queue.size(); }
}
