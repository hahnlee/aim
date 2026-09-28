package dev.aim.probe;

import java.io.File;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;

public final class UnixFileSystemDarwinSmoke {
    private static final int BA_EXISTS = 0x01;
    private static final int BA_REGULAR = 0x02;
    private static final int BA_DIRECTORY = 0x04;
    private static final int ACCESS_READ = 0x04;
    private static final int ACCESS_WRITE = 0x02;

    private static native void initIDs();
    private static native String canonicalize0(String path, boolean targetSdk35);
    private static native int getBooleanAttributes0(String path);
    private static native long getNameMax0(String path);
    private static native boolean setPermission0(
            File file, int access, boolean enable, boolean ownerOnly);
    private static native long getLastModifiedTime0(File file);
    private static native boolean createFileExclusively0(String path);
    private static native String[] list0(File file);
    private static native boolean createDirectory0(File file);
    private static native boolean setLastModifiedTime0(File file, long timeMillis);
    private static native boolean setReadOnly0(File file);
    private static native long getSpace0(File file, int kind);

    private static void require(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            throw new IllegalArgumentException("managed JNI dylib required");
        }
        System.load(args[0]);
        initIDs();

        Path rootPath = Files.createTempDirectory("aim-unixfs-");
        File root = rootPath.toFile();
        File child = new File(root, "child");
        File alpha = new File(child, "alpha.txt");
        File beta = new File(child, "beta.txt");
        try {
            require(createDirectory0(child), "createDirectory0");
            require(createFileExclusively0(alpha.getAbsolutePath()),
                    "createFileExclusively0 alpha");
            require(createFileExclusively0(beta.getAbsolutePath()),
                    "createFileExclusively0 beta");
            require(!createFileExclusively0(alpha.getAbsolutePath()),
                    "exclusive duplicate accepted");

            int childAttributes = getBooleanAttributes0(child.getAbsolutePath());
            int fileAttributes = getBooleanAttributes0(alpha.getAbsolutePath());
            require((childAttributes & (BA_EXISTS | BA_DIRECTORY)) ==
                            (BA_EXISTS | BA_DIRECTORY),
                    "directory attributes=" + childAttributes);
            require((fileAttributes & (BA_EXISTS | BA_REGULAR)) ==
                            (BA_EXISTS | BA_REGULAR),
                    "file attributes=" + fileAttributes);

            String[] names = list0(child);
            require(names != null, "list0 returned null");
            Arrays.sort(names);
            require(Arrays.equals(names, new String[]{"alpha.txt", "beta.txt"}),
                    "list0=" + Arrays.toString(names));

            String canonical = canonicalize0(
                    new File(child, "../child/./alpha.txt").getAbsolutePath(), true);
            require(alpha.getCanonicalPath().equals(canonical),
                    "canonicalize0=" + canonical);
            require(getNameMax0(child.getAbsolutePath()) > 0, "getNameMax0");

            long requestedTime = 1_700_000_123_000L;
            require(setLastModifiedTime0(alpha, requestedTime),
                    "setLastModifiedTime0");
            require(Math.abs(getLastModifiedTime0(alpha) - requestedTime) < 2_000L,
                    "getLastModifiedTime0");
            require(setPermission0(alpha, ACCESS_READ, true, true),
                    "setPermission0 read");
            require(setReadOnly0(alpha), "setReadOnly0");
            require(setPermission0(alpha, ACCESS_WRITE, true, true),
                    "setPermission0 restore write");

            long total = getSpace0(child, 0);
            long free = getSpace0(child, 1);
            long usable = getSpace0(child, 2);
            require(total > 0 && free > 0 && usable > 0,
                    "space total=" + total + " free=" + free + " usable=" + usable);

            System.out.println(
                    "managed-unixfs: methods=12 list0=alpha.txt,beta.txt "
                            + "canonicalize=pass attributes=pass permissions=pass space=pass");
        } finally {
            Files.deleteIfExists(alpha.toPath());
            Files.deleteIfExists(beta.toPath());
            Files.deleteIfExists(child.toPath());
            Files.deleteIfExists(rootPath);
        }
    }
}
