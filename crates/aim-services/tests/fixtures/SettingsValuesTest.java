package dev.aim.server;

import android.os.Binder;

/** ContentValues.getAsString / libcore FloatingDecimal boundary cases. */
public final class SettingsValuesTest {
    private static void check(Object value, String expected) {
        if (!expected.equals(SettingsValues.asString(value)))
            throw new AssertionError(value + " expected " + expected);
    }
    public static void main(String[] args) {
        check(1e7f, "1.0E7"); check(1.5f, "1.5"); check(0.1f, "0.1");
        check(1e-3f, "0.001"); check(1e-4f, "1.0E-4"); check(-0.0f, "-0.0");
        check(Float.NaN, "NaN"); check(Float.POSITIVE_INFINITY, "Infinity");
        check(Float.MIN_VALUE, "1.4E-45"); check(Float.MAX_VALUE, "3.4028235E38");
        check(1e7, "1.0E7"); check(1.5, "1.5"); check(0.1, "0.1");
        check(1e-3, "0.001"); check(1e-4, "1.0E-4"); check(-0.0, "-0.0");
        check(Double.NaN, "NaN"); check(Double.NEGATIVE_INFINITY, "-Infinity");
        check(Double.MIN_VALUE, "4.9E-324"); check(Double.MAX_VALUE, "1.7976931348623157E308");
        for (byte[] bytes : new byte[][] {new byte[0], new byte[] {1, 2, -1}})
            check(bytes, "[B@" + Integer.toHexString(System.identityHashCode(bytes)));
        if (SettingsValues.asString(null) != null) throw new AssertionError("null");
        SettingsValues bridge = new SettingsValues();
        for (int uid : new int[] {0, 2000, 10100, 101000}) {
            Binder.uid = uid;
            try {
                bridge.getAsString(new byte[0]);
                throw new AssertionError("non-system conversion");
            } catch (SecurityException expected) {}
        }
    }
}
