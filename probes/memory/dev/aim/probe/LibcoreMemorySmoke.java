package dev.aim.probe;

import java.util.Arrays;
import libcore.io.Memory;

public final class LibcoreMemorySmoke {
  private static native long allocate(int byteCount);
  private static native void free(long address);

  private static void require(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }

  public static void main(String[] args) {
    System.load(args[0]);
    long address = allocate(256);
    try {
      Memory.pokeByte(address + 1, (byte) 0x7f);
      require(Memory.peekByte(address + 1) == (byte) 0x7f,
              "peekByte/pokeByte failed");

      Memory.pokeShortNative(address + 1, (short) 0x1234);
      require(Memory.peekShortNative(address + 1) == (short) 0x1234,
              "unaligned short failed");
      Memory.pokeIntNative(address + 3, 0x12345678);
      require(Memory.peekIntNative(address + 3) == 0x12345678,
              "unaligned int failed");
      Memory.pokeLongNative(address + 5, 0x1122334455667788L);
      require(Memory.peekLongNative(address + 5) == 0x1122334455667788L,
              "unaligned long failed");

      int[] ints = {0x01020304, 0x11223344};
      Memory.pokeIntArray(address + 1, ints, 0, ints.length, false);
      require(Memory.peekIntNative(address + 1) == ints[0] &&
                  Memory.peekIntNative(address + 5) == ints[1],
              "unswapped int array failed");
      Memory.pokeIntArray(address + 1, ints, 0, ints.length, true);
      require(Memory.peekIntNative(address + 1) ==
                  Integer.reverseBytes(ints[0]) &&
                  Memory.peekIntNative(address + 5) ==
                  Integer.reverseBytes(ints[1]),
              "swapped int array failed");

      short[] shorts = {(short) 0x1122, (short) 0x3344, (short) 0x5566};
      Memory.pokeShortArray(address + 1, shorts, 0, shorts.length, true);
      for (int i = 0; i < shorts.length; ++i) {
        require(Memory.peekShortNative(address + 1 + i * 2L) ==
                    Short.reverseBytes(shorts[i]),
                "swapped odd short array failed at " + i);
      }

      char[] chars = {(char) 0x1234};
      Memory.pokeCharArray(address + 1, chars, 0, 1, true);
      require(Memory.peekShortNative(address + 1) ==
                  Short.reverseBytes((short) chars[0]),
              "swapped char array failed");
      float[] floats = {1.0f};
      Memory.pokeFloatArray(address + 1, floats, 0, 1, true);
      require(Memory.peekIntNative(address + 1) ==
                  Integer.reverseBytes(Float.floatToRawIntBits(floats[0])),
              "swapped float array failed");
      long[] longs = {0x0102030405060708L};
      Memory.pokeLongArray(address + 1, longs, 0, 1, true);
      require(Memory.peekLongNative(address + 1) ==
                  Long.reverseBytes(longs[0]),
              "swapped long array failed");
      double[] doubles = {Math.PI};
      Memory.pokeDoubleArray(address + 1, doubles, 0, 1, true);
      require(Memory.peekLongNative(address + 1) == Long.reverseBytes(
                  Double.doubleToRawLongBits(doubles[0])),
              "swapped double array failed");

      byte[] source = {9, 10, 11, 12, 13};
      Memory.pokeByteArray(address + 20, source, 1, 3);
      require(Memory.peekByte(address + 20) == 10 &&
                  Memory.peekByte(address + 21) == 11 &&
                  Memory.peekByte(address + 22) == 12,
              "byte array offset copy failed");

      byte[] nativeOrder = new byte[ints.length * Integer.BYTES];
      Memory.unsafeBulkPut(nativeOrder, 0, nativeOrder.length, ints, 0,
                           Integer.BYTES, false);
      int[] roundTrip = new int[ints.length];
      Memory.unsafeBulkGet(roundTrip, 0, nativeOrder.length, nativeOrder, 0,
                           Integer.BYTES, false);
      require(Arrays.equals(roundTrip, ints), "native-order bulk copy failed");

      byte[] swapped = new byte[ints.length * Integer.BYTES];
      Memory.unsafeBulkPut(swapped, 0, swapped.length, ints, 0,
                           Integer.BYTES, true);
      require((swapped[0] & 0xff) == 0x01 && (swapped[1] & 0xff) == 0x02 &&
                  (swapped[2] & 0xff) == 0x03 &&
                  (swapped[3] & 0xff) == 0x04,
              "swapped bulk byte order failed");
      Arrays.fill(roundTrip, 0);
      Memory.unsafeBulkGet(roundTrip, 0, swapped.length, swapped, 0,
                           Integer.BYTES, true);
      require(Arrays.equals(roundTrip, ints), "swapped bulk round-trip failed");

      System.out.println(
          "managed-libcore-memory: methods=18+7 byte=pass unaligned=pass endian=pass arrays=pass bulk=pass");
    } finally {
      free(address);
    }
  }
}
