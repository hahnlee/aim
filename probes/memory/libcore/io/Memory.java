package libcore.io;

public final class Memory {
  private Memory() {}

  // libcore/luni/src/main/native/libcore_io_Memory.cpp (18 methods).
  public static native void memmove(Object dst, int dstOffset, Object src,
                                    int srcOffset, long byteCount);
  public static native byte peekByte(long address);
  public static native int peekIntNative(long address);
  public static native long peekLongNative(long address);
  public static native short peekShortNative(long address);
  public static native void pokeByte(long address, byte value);
  public static native void pokeByteArray(long address, byte[] src, int offset,
                                          int length);
  public static native void pokeCharArray(long address, char[] src, int offset,
                                          int count, boolean swap);
  public static native void pokeDoubleArray(long address, double[] src,
                                            int offset, int count,
                                            boolean swap);
  public static native void pokeFloatArray(long address, float[] src,
                                           int offset, int count,
                                           boolean swap);
  public static native void pokeIntNative(long address, int value);
  public static native void pokeIntArray(long address, int[] src, int offset,
                                         int count, boolean swap);
  public static native void pokeLongNative(long address, long value);
  public static native void pokeLongArray(long address, long[] src, int offset,
                                          int count, boolean swap);
  public static native void pokeShortNative(long address, short value);
  public static native void pokeShortArray(long address, short[] src,
                                           int offset, int count,
                                           boolean swap);
  public static native void unsafeBulkGet(Object dst, int dstOffset,
                                          int byteCount, byte[] src,
                                          int srcOffset, int sizeofElement,
                                          boolean swap);
  public static native void unsafeBulkPut(byte[] dst, int dstOffset,
                                          int byteCount, Object src,
                                          int srcOffset, int sizeofElement,
                                          boolean swap);

  // Complementary ART runtime owner (7 methods); declared to preserve the
  // complete Android managed surface but intentionally not registered here.
  public static native void peekByteArray(long address, byte[] dst,
                                          int dstOffset, int byteCount);
  public static native void peekCharArray(long address, char[] dst,
                                          int dstOffset, int count,
                                          boolean swap);
  public static native void peekDoubleArray(long address, double[] dst,
                                            int dstOffset, int count,
                                            boolean swap);
  public static native void peekFloatArray(long address, float[] dst,
                                           int dstOffset, int count,
                                           boolean swap);
  public static native void peekIntArray(long address, int[] dst,
                                         int dstOffset, int count,
                                         boolean swap);
  public static native void peekLongArray(long address, long[] dst,
                                          int dstOffset, int count,
                                          boolean swap);
  public static native void peekShortArray(long address, short[] dst,
                                           int dstOffset, int count,
                                           boolean swap);
}
