package android.os;

/** Test-only HandlerThread with a deterministic Looper token. */
public final class HandlerThread {
    private final Looper looper = Looper.getMainLooper();

    public HandlerThread(String name) {}
    public void start() {}
    public Looper getLooper() { return looper; }
    public boolean quitSafely() { return true; }
}
