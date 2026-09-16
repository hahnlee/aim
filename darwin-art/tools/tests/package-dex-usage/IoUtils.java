package libcore.io;
public final class IoUtils {
    public static void closeQuietly(java.lang.AutoCloseable input) {
        if (input != null) try { input.close(); } catch (Exception ignored) {}
    }
}
