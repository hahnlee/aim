package dev.aim.probe;

import java.io.File;
import java.io.FileDescriptor;
import java.io.FileOutputStream;

public final class FileDescriptorDarwinSmoke {
    private static native void sync(FileDescriptor descriptor) throws java.io.SyncFailedException;
    private static native boolean getAppend(FileDescriptor descriptor);
    private static native boolean isSocket(FileDescriptor descriptor);
    private static native boolean socketPair();

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            throw new IllegalArgumentException("expected native library path");
        }
        System.load(args[0]);
        File file = File.createTempFile("aim-file-descriptor-", ".tmp");
        try {
            try (FileOutputStream stream = new FileOutputStream(file, true)) {
                stream.write(0x41);
                sync(stream.getFD());
                if (!getAppend(stream.getFD()) || isSocket(stream.getFD())) {
                    throw new AssertionError("append/socket descriptor classification failed");
                }
            }
            try (FileOutputStream stream = new FileOutputStream(file, false)) {
                if (getAppend(stream.getFD())) {
                    throw new AssertionError("non-append descriptor reported O_APPEND");
                }
            }
            if (!socketPair()) {
                throw new AssertionError("socketpair descriptor was not recognized");
            }
        } finally {
            if (!file.delete()) {
                file.deleteOnExit();
            }
        }
        System.out.println("managed-file-descriptor: methods=3 sync=pass socket=pass append=pass");
    }
}
