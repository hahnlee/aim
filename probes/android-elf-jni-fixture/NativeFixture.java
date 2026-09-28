package darwin.art.nativefixture;

public final class NativeFixture {
    private NativeFixture() {}

    public static native long nativeAdd(int left, long middle, int right);

    public static native long nativeSpill(
            boolean z,
            byte b,
            char c,
            short s,
            int i,
            long j,
            Object reference,
            float f0,
            double d0,
            float f1,
            double d1,
            float f2,
            double d2,
            float f3,
            double d3,
            float f4,
            float f5,
            double d4);

    public static native int nativeUsesEnv();

    public static native int nativeNarrowStack(
            int a0,
            int a1,
            int a2,
            int a3,
            int a4,
            int a5,
            boolean z,
            byte b,
            char c,
            short s,
            int i,
            long j,
            Object reference);

    public static native Object nativeEcho(Object value);

    public static native float nativeFloat(float value);

    public static native double nativeDouble(double value);

    public static native void nativeVoid();

    public static int runAcceptance() {
        if (nativeAdd(10, 20L, 12) != 42L) {
            return -1;
        }
        if (nativeUsesEnv() != 42) {
            return -3;
        }
        Object reference = new Object();
        if (nativeNarrowStack(
                        10,
                        11,
                        12,
                        13,
                        14,
                        15,
                        true,
                        (byte) 0x81,
                        (char) 0xabcd,
                        (short) 0x8765,
                        0x45678923,
                        0x2233445566778899L,
                        reference)
                != 42) {
            return -4;
        }
        if (nativeEcho(reference) != reference
                || nativeFloat(1.25f) != 1.75f
                || nativeDouble(2.5) != 2.75) {
            return -5;
        }
        nativeVoid();
        long digest = nativeSpill(
                true,
                (byte) 0x81,
                (char) 0xabcd,
                (short) 0x8765,
                0x45678923,
                0x2233445566778899L,
                new Object(),
                1.0f,
                1.5,
                2.0f,
                2.5,
                3.0f,
                3.5,
                4.0f,
                4.5,
                5.0f,
                6.0f,
                5.5);
        return digest == 0x679c0e434597370eL ? 42 : -2;
    }
}
