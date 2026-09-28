package android.os;

/** Deterministic Handler facade; delay is intentionally not wall-clock based. */
public class Handler {
    private final Looper looper;

    public Handler(Looper looper) {
        if (looper == null) throw new IllegalArgumentException("missing looper");
        this.looper = looper;
    }

    public boolean postDelayed(Runnable task, long delayMillis) {
        if (task == null || delayMillis < 0) return false;
        return looper.owner.post(task);
    }
}
