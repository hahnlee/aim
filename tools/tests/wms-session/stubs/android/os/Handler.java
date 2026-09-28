package android.os;

public class Handler {
    /** -1 disables injection; zero rejects the next post, positive allows that many. */
    public static int allowPosts = -1;
    private final Looper looper;
    public Handler(Looper value) { looper = value; }
    public boolean postDelayed(Runnable task, long delayMillis) {
        if (task == null || delayMillis < 0) return false;
        if (allowPosts == 0) return false;
        if (allowPosts > 0) --allowPosts;
        return looper.owner.post(task);
    }
}
