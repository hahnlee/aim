package dev.aim.probe;

import dev.aim.managedload.ManagedNativeLoad;

public final class Hello {
    public static native int hostPageSize();

    private static native long nativePackedIntegerStack(
            int a0,
            int a1,
            int a2,
            int a3,
            int a4,
            int a5,
            int spilled,
            long wide,
            int tail0,
            int tail1);

    private static native long nativePackedFloatingStack(
            float a0,
            float a1,
            float a2,
            float a3,
            float a4,
            float a5,
            float a6,
            float a7,
            float spilled,
            double wide);

    private static native long nativePackedReferenceStack(
            int a0,
            int a1,
            int a2,
            int a3,
            int a4,
            int a5,
            int spilled,
            Object reference,
            int tail);

    private static native long nativePackedNarrowStack(
            int a0,
            int a1,
            int a2,
            int a3,
            int a4,
            int a5,
            boolean boolValue,
            byte byteValue,
            char charValue,
            short shortValue,
            int intValue,
            long longValue);

    public static void main(String[] args) {
        String nativeFixture = System.getenv("AIM_MANAGED_NATIVE_FIXTURE");
        if (nativeFixture == null) {
            throw new IllegalStateException("managed native fixture path is missing");
        }
        int result = ManagedNativeLoad.loadAbsolute(nativeFixture);
        if (result != 42) {
            throw new AssertionError("managed native result=" + result);
        }
        System.out.println(args[0]);
    }

    public static int answer() {
        return 42;
    }

    public static int nativeRoundTrip() {
        return hostPageSize() == 16384 ? 42 : -1;
    }

    public static int nativeStackPcsRoundTrip() {
        long integer =
                nativePackedIntegerStack(
                        10,
                        11,
                        12,
                        13,
                        14,
                        15,
                        0x10203040,
                        0x1122334455667788L,
                        0x50607080,
                        0x12345678);
        long floating =
                nativePackedFloatingStack(
                        1.0f, 2.0f, 3.0f, 4.0f, 5.0f, 6.0f, 7.0f, 8.0f, 9.5f, 10.25);
        Object marker = new Object();
        long reference =
                nativePackedReferenceStack(
                        20, 21, 22, 23, 24, 25, 0x23456701, marker, 0x34567812);
        long narrow =
                nativePackedNarrowStack(
                        30,
                        31,
                        32,
                        33,
                        34,
                        35,
                        true,
                        (byte) 0x81,
                        (char) 0xabcd,
                        (short) 0x8765,
                        0x45678923,
                        0x2233445566778899L);
        if (integer != 0x13579bdf2468ace0L) return -10;
        if (floating != 0x02468ace13579bdfL) return -11;
        if (reference != 0x55aa55aa33cc33ccL) return -12;
        if (narrow != 0x1122aabb3344ccddL) return -13;
        return 42;
    }

    public static int runtimeNativeArraycopy() {
        int[] source = new int[] {19, 23};
        int[] destination = new int[2];
        System.arraycopy(source, 0, destination, 0, source.length);
        return destination[0] + destination[1];
    }
}
