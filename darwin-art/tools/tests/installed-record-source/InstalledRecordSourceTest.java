import java.io.IOException;

/** Real JVM/JNI boundary test for the production installed-record source. */
public final class InstalledRecordSourceTest {
    private interface Call {
        String run();
    }

    private static native String query(String profileSocket, String packageName);

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void expect(Class<? extends Throwable> type, Call call) {
        try {
            call.run();
            throw new AssertionError("expected " + type.getName());
        } catch (Throwable error) {
            check(type.isInstance(error), "expected " + type.getName() + ", got " + error);
        }
    }

    public static void main(String[] args) {
        check(args.length == 1, "usage: InstalledRecordSourceTest LIBRARY");
        System.load(args[0]);

        String socket = "/private/tmp/darwin-art-record-source/explicit.sock";
        String normal = query(socket, "normal");
        check(normal.equals("darwin-art-launch-v1\n"
                + "apk=/installed/base.apk\n"
                + "app_id=10042\n"
                + "metadata=socket=" + socket + " package=normal\n"),
                "explicit socket/package were not forwarded unchanged: " + normal);

        String emoji = "\uD83D\uDE00";
        String unicode = query(socket, "unicode");
        check(unicode.equals("darwin-art-launch-v1\n"
                + "apk=/packages/" + emoji + "/base.apk\n"
                + "metadata=socket=" + socket + " package=unicode\n"),
                "UTF-8 record was not decoded through String(byte[], UTF-8): " + unicode);

        check(query(socket, "missing") == null, "status 1 did not map to null");
        expect(IOException.class, () -> query(socket, "error"));
        expect(IllegalStateException.class, () -> query(null, "normal"));
        expect(IllegalArgumentException.class, () -> query(socket, null));
        System.out.println("InstalledRecordSource JNI boundary PASS (real JVM/JNI dylib)");
    }
}
