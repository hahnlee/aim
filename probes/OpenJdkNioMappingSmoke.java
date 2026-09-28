package dev.aim.probe;

import java.io.FileDescriptor;
import java.io.FileInputStream;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

final class NioFileDispatcher {
  static native void closeIntFD(int fd);
  static native void preClose0(FileDescriptor fd);
  static native void close0(FileDescriptor fd);
  static native void release0(FileDescriptor fd, long position, long size);
  static native int lock0(FileDescriptor fd, boolean block, long position,
                          long size, boolean shared);
  static native long size0(FileDescriptor fd);
  static native int truncate0(FileDescriptor fd, long size);
  static native int force0(FileDescriptor fd, boolean metadata);
  static native long writev0(FileDescriptor fd, long address, int length);
  static native int pwrite0(FileDescriptor fd, long address, int length,
                            long offset);
  static native int write0(FileDescriptor fd, long address, int length);
  static native long readv0(FileDescriptor fd, long address, int length);
  static native int pread0(FileDescriptor fd, long address, int length,
                           long offset);
  static native int read0(FileDescriptor fd, long address, int length);
}

final class NioFileChannel {
  private final FileDescriptor fd;

  NioFileChannel(FileDescriptor fd) {
    this.fd = fd;
  }

  private static native long initIDs();
  private native long map0(int protection, long offset, long length);
  private static native int unmap0(long address, long length);
  private native long position0(FileDescriptor fd, long offset);
  private native long transferTo0(FileDescriptor source, long position,
                                  long count, FileDescriptor destination);

  long size() {
    return NioFileDispatcher.size0(fd);
  }

  long mapReadOnly(long length) {
    return map0(0, 0, length);
  }

  void unmap(long address, long length) {
    int result = unmap0(address, length);
    if (result != 0) {
      throw new AssertionError("munmap returned " + result);
    }
  }
}

public final class OpenJdkNioMappingSmoke {
  private static native int peek(long address);
  private static native int restoreSignalHandler();
  private static native int virtualReadv(FileDescriptor fd);

  private static void require(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }

  public static void main(String[] args) throws Exception {
    System.load(args[0]);
    byte[] content = "android16-nio-map".getBytes(StandardCharsets.UTF_8);
    Path path = Files.createTempFile("aim-nio", ".tmp");
    Files.write(path, content);

    try (FileInputStream stream = new FileInputStream(path.toFile())) {
      NioFileChannel channel = new NioFileChannel(stream.getFD());
      require(channel.size() == content.length, "FileChannel size mismatch");
      long address = channel.mapReadOnly(content.length);
      require(address != 0, "read-only mmap returned null");
      require(peek(address) == (content[0] & 0xff),
              "mapped byte differs from file content");
      channel.unmap(address, content.length);
    }

    FileDescriptor virtual = new FileDescriptor();
    var fdField = FileDescriptor.class.getDeclaredField("fd");
    fdField.setAccessible(true);
    fdField.setInt(virtual, 0x30000001);
    require(virtualReadv(virtual) == 3,
            "production virtual-FD readv did not preserve one vector call");

    Class<?> nativeThread = Class.forName("sun.nio.ch.NativeThread");
    Method current = nativeThread.getDeclaredMethod("current");
    Method signal = nativeThread.getDeclaredMethod("signal", long.class);
    current.setAccessible(true);
    signal.setAccessible(true);
    long thread = (Long) current.invoke(null);
    require(thread != 0, "NativeThread.current returned zero");
    signal.invoke(null, thread);
    int restoreStatus = restoreSignalHandler();
    require(restoreStatus == 1,
            "NativeThread did not restore SIGIO status=" + restoreStatus);

    Files.delete(path);
    System.out.println(
        "managed-openjdk-nio: methods=5+14+2 size=pass map-ro=pass unmap=pass "
            + "virtual-readv=pass thread=pass signal-restore=pass");
  }
}
