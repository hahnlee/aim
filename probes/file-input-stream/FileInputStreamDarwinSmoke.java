package dev.aim.probe;

import java.io.FileInputStream;
import java.io.IOException;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;

public final class FileInputStreamDarwinSmoke {
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

        byte[] contents = "0123456789abcdef".getBytes(StandardCharsets.US_ASCII);
        Path path = Files.createTempFile("aim-fis-", ".bin");
        Files.write(path, contents);
        try {
            FileInputStream input = new FileInputStream(path.toFile());
            Method length0 = FileInputStream.class.getDeclaredMethod("length0");
            Method position0 = FileInputStream.class.getDeclaredMethod("position0");
            length0.setAccessible(true);
            position0.setAccessible(true);

            require((long) length0.invoke(input) == contents.length, "length0");
            require((long) position0.invoke(input) == 0, "position0 initial");
            require(input.available() == contents.length, "available initial");
            require(input.read() == contents[0], "read single");

            byte[] pair = new byte[2];
            require(input.read(pair) == 2, "read pair count");
            require(Arrays.equals(pair, new byte[]{contents[1], contents[2]}),
                    "read pair bytes");
            require(input.skip(4) == 4, "skip0");
            require((long) position0.invoke(input) == 7, "position0 after skip");
            require(input.available() == contents.length - 7, "available after skip");

            input.close();
            boolean closedRejected = false;
            try {
                input.available();
            } catch (IOException expected) {
                closedRejected = true;
            }
            require(closedRejected, "closed stream available accepted");
            System.out.println(
                    "managed-file-input-stream: methods=4 open=pass read=pass "
                            + "length=pass position=pass available=pass skip=pass close=pass");
        } finally {
            Files.deleteIfExists(path);
        }
    }
}
