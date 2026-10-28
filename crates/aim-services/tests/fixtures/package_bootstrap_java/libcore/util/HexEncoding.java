package libcore.util;
public final class HexEncoding {
    public static String encodeToString(byte[] bytes, boolean uppercase) {
        var result = java.util.HexFormat.of().formatHex(bytes);
        return uppercase ? result.toUpperCase(java.util.Locale.ROOT) : result;
    }
}
