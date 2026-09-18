package android.os;

/** Deterministic fixture looper; work runs only when the test advances it. */
public final class Looper {
    final HandlerThread owner;

    Looper(HandlerThread owner) {
        this.owner = owner;
    }
}
