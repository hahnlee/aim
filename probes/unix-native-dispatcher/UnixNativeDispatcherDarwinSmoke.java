package dev.aim.probe;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Comparator;

public final class UnixNativeDispatcherDarwinSmoke {
    private static final int SUPPORTS_OPENAT = 2;
    private static final int SUPPORTS_FUTIMES = 4;
    private static final int SUPPORTS_BIRTHTIME = 65536;

    private static native int run(String root);

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            throw new IllegalArgumentException("expected native library path");
        }
        System.load(args[0]);
        Path root = Files.createTempDirectory("aim-und-");
        int capabilities;
        try {
            capabilities = run(root.toString());
        } finally {
            if (Files.exists(root)) {
                try (var paths = Files.walk(root)) {
                    paths.sorted(Comparator.reverseOrder()).forEach(path -> {
                        try {
                            Files.deleteIfExists(path);
                        } catch (Exception ignored) {
                        }
                    });
                }
            }
        }
        if (capabilities < 0 || (capabilities & SUPPORTS_FUTIMES) == 0 ||
                (capabilities & SUPPORTS_BIRTHTIME) == 0) {
            throw new AssertionError("Darwin capability contract failed: " + capabilities);
        }
        if ((capabilities & SUPPORTS_OPENAT) != 0) {
            throw new AssertionError("openat group must remain disabled without futimesat");
        }
        System.out.println("managed-unix-native-dispatcher: methods=47 init=pass posix-io=pass directory=pass links=pass identity=pass capabilities=" + capabilities);
    }
}
