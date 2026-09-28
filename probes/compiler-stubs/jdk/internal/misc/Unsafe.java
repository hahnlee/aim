package jdk.internal.misc;

import java.lang.reflect.Field;

// Compile-time view of Android's boot-class-path Unsafe. This class is never
// packaged into an application DEX; the runtime resolves the real libcore
// implementation from core-oj.
public final class Unsafe {
  private Unsafe() {}
  private static final Unsafe theUnsafe = new Unsafe();

  // Keep these as runtime field reads, just like libcore. Literal initializers
  // would be folded into the test DEX and incorrectly replace Android's real
  // array layout with values from this signature-only class.
  public static final int ARRAY_INT_BASE_OFFSET =
      theUnsafe.arrayBaseOffset(int[].class);
  public static final int ARRAY_LONG_BASE_OFFSET =
      theUnsafe.arrayBaseOffset(long[].class);
  public static final int ARRAY_FLOAT_BASE_OFFSET =
      theUnsafe.arrayBaseOffset(float[].class);
  public static final int ARRAY_DOUBLE_BASE_OFFSET =
      theUnsafe.arrayBaseOffset(double[].class);

  public int arrayBaseOffset(Class<?> type) { return 0; }
  public int arrayIndexScale(Class<?> type) { return 0; }
  public long objectFieldOffset(Field field) { return 0; }
  public long objectFieldOffset(Class<?> type, String field) { return 0; }

  public native boolean compareAndSwapInt(Object object, long offset, int expected, int value);
  public native boolean compareAndSwapLong(Object object, long offset, long expected, long value);
  public native boolean compareAndSwapObject(Object object, long offset, Object expected, Object value);
  public native boolean compareAndSetInt(Object object, long offset, int expected, int value);
  public native boolean compareAndSetLong(Object object, long offset, long expected, long value);
  public native boolean compareAndSetObject(Object object, long offset, Object expected, Object value);
  public native boolean compareAndSetReference(Object object, long offset, Object expected, Object value);

  public native void copyMemory(Object source, long sourceOffset, Object destination,
                                long destinationOffset, long bytes);
  public native void copyMemory(long source, long destination, long bytes);

  public native boolean getBooleanVolatile(Object object, long offset);
  public native byte getByte(Object object, long offset);
  public native byte getByte(long address);
  public native byte getByteVolatile(Object object, long offset);
  public native char getCharVolatile(Object object, long offset);
  public native double getDoubleVolatile(Object object, long offset);
  public native float getFloatVolatile(Object object, long offset);
  public native int getInt(Object object, long offset);
  public native int getIntAcquire(Object object, long offset);
  public native int getIntVolatile(Object object, long offset);
  public native long getLong(Object object, long offset);
  public native long getLongAcquire(Object object, long offset);
  public native long getLongVolatile(Object object, long offset);
  public native Object getObject(Object object, long offset);
  public native Object getObjectAcquire(Object object, long offset);
  public native Object getReferenceVolatile(Object object, long offset);
  public native short getShortVolatile(Object object, long offset);

  public native void putBooleanVolatile(Object object, long offset, boolean value);
  public native void putByteVolatile(Object object, long offset, byte value);
  public native void putCharVolatile(Object object, long offset, char value);
  public native void putDoubleVolatile(Object object, long offset, double value);
  public native void putFloatVolatile(Object object, long offset, float value);
  public native void putInt(Object object, long offset, int value);
  public native void putIntRelease(Object object, long offset, int value);
  public native void putIntVolatile(Object object, long offset, int value);
  public native void putLong(Object object, long offset, long value);
  public native void putLongRelease(Object object, long offset, long value);
  public native void putLongVolatile(Object object, long offset, long value);
  public native void putObject(Object object, long offset, Object value);
  public native void putObjectRelease(Object object, long offset, Object value);
  public native void putReferenceVolatile(Object object, long offset, Object value);
  public native void putShortVolatile(Object object, long offset, short value);
}
