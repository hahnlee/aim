package android.os;

/** Test-only Handler that deterministically drains posted callbacks. */
public final class Handler {
    public Handler(Looper looper) {}

    public boolean post(Runnable runnable) {
        runnable.run();
        return true;
    }

    public boolean postDelayed(Runnable runnable, long delayMillis) { return true; }

    public void removeCallbacksAndMessages(Object token) {}
}
