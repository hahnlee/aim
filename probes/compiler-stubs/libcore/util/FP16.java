package libcore.util;

/** Signature-only view of the hidden libcore FP16 API for javac. */
public final class FP16 {
    public static final int SIZE = 16;
    public static final int MAX_EXPONENT = 15;
    public static final int MIN_EXPONENT = -14;
    public static final short EPSILON = (short) 0x1400;
    public static final short LOWEST_VALUE = (short) 0xfbff;
    public static final short MAX_VALUE = (short) 0x7bff;
    public static final short MIN_NORMAL = (short) 0x0400;
    public static final short MIN_VALUE = (short) 0x0001;
    public static final short NaN = (short) 0x7e00;
    public static final short NEGATIVE_INFINITY = (short) 0xfc00;
    public static final short NEGATIVE_ZERO = (short) 0x8000;
    public static final short POSITIVE_INFINITY = (short) 0x7c00;
    public static final short POSITIVE_ZERO = (short) 0x0000;
    public static final int SIGN_SHIFT = 15;
    public static final int EXPONENT_SHIFT = 10;
    public static final int SIGN_MASK = 0x8000;
    public static final int SHIFTED_EXPONENT_MASK = 0x1f;
    public static final int SIGNIFICAND_MASK = 0x3ff;
    public static final int EXPONENT_SIGNIFICAND_MASK = 0x7fff;
    public static final int EXPONENT_BIAS = 15;

    private FP16() {}

    public static int compare(short x, short y) { return 0; }
    public static short rint(short h) { return 0; }
    public static short ceil(short h) { return 0; }
    public static short floor(short h) { return 0; }
    public static short trunc(short h) { return 0; }
    public static short min(short x, short y) { return 0; }
    public static short max(short x, short y) { return 0; }
    public static boolean less(short x, short y) { return false; }
    public static boolean lessEquals(short x, short y) { return false; }
    public static boolean greater(short x, short y) { return false; }
    public static boolean greaterEquals(short x, short y) { return false; }
    public static boolean equals(short x, short y) { return false; }
    public static boolean isNaN(short h) { return false; }
    public static boolean isInfinite(short h) { return false; }
    public static boolean isNormalized(short h) { return false; }
    public static float toFloat(short h) { return 0.0f; }
    public static short toHalf(float f) { return 0; }
    public static String toHexString(short h) { return ""; }
}
