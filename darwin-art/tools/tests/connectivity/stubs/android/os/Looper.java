package android.os;

/** Test-only main-looper token. */
public final class Looper {
    private static final Looper MAIN = new Looper();

    public static Looper getMainLooper() {
        return MAIN;
    }
}
