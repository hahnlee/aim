package java.lang;

import java.io.UnsupportedEncodingException;
import java.nio.charset.Charset;

/** Signature-only view of the hidden libcore StringFactory API for javac. */
public final class StringFactory {
    private StringFactory() {}

    public static String newEmptyString() { return null; }
    public static String newStringFromBytes(byte[] data) { return null; }
    public static String newStringFromBytes(byte[] data, byte coder) { return null; }
    public static native String newStringFromUtf16Bytes(byte[] data, int offset,
                                                         int charCount);
    public static String newStringFromBytes(byte[] data, int high) { return null; }
    public static String newStringFromBytes(byte[] data, int offset, int byteCount) {
        return null;
    }
    public static native String newStringFromBytes(byte[] data, int high,
                                                    int offset, int byteCount);
    public static String newStringFromBytes(byte[] data, int offset, int byteCount,
                                             String charsetName)
            throws UnsupportedEncodingException { return null; }
    public static String newStringFromBytes(byte[] data, String charsetName)
            throws UnsupportedEncodingException { return null; }
    public static String newStringFromBytes(byte[] data, int offset, int byteCount,
                                             Charset charset) { return null; }
    public static String newStringFromBytes(byte[] data, Charset charset) { return null; }
    public static String newStringFromChars(char[] data) { return null; }
    public static String newStringFromChars(char[] data, int offset, int charCount) {
        return null;
    }
    static native String newStringFromChars(int offset, int charCount, char[] data);
    public static native String newStringFromString(String toCopy);
    public static native String newStringFromUtf8Bytes(byte[] data, int offset, int byteCount);
    public static String newStringFromStringBuffer(StringBuffer stringBuffer) { return null; }
    public static String newStringFromCodePoints(int[] codePoints, int offset, int count) {
        return null;
    }
    public static String newStringFromStringBuilder(StringBuilder stringBuilder) { return null; }
}
