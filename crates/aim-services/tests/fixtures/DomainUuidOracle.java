/** Uses the original UUID.fromString under each original compatibility mode. */
final class DomainUuidOracle {
    static void verify(java.io.File directory) throws Exception {
        if (dalvik.system.VMRuntime.getSdkVersion() < 34) throw new AssertionError("UUID strict runtime unavailable");
        var out = android.os.Parcel.obtain();
        try {
            for (int c = 0; c <= 65535; c++) out.writeInt(Character.digit((char)c, 16));
            java.nio.file.Files.write(new java.io.File(directory, "domain-uuid-digits.original").toPath(), out.marshall());
            out.recycle(); out = android.os.Parcel.obtain();
            String[] values = {"", "1-2-3-4-5", "1-2-3-4-5-", "1-2-3-4-5--", "1-2-3-4", "1-2-3-4-5-6", "-1-2-3-4", "1--2-3-4", "1-2-3-4-", "+1-2-3-4-5", "1-+2-3-4-5", "0-10000-0-0-0", "fffffffff-1-2-3-4", "7fffffffffffffff-1-2-3-4", "8000000000000000-1-2-3-4", "gg-1-2-3-4", "0x1-2-3-4-5", " 1-2-3-4-5", "١-٢-٣-٤-٥", "ＦＦ-１-２-３-４", "𐒠-1-2-3-4", "00000000-0000-0000-0000-000000000001", "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF", "00000000-0000-0000-0000-000000000001x"};
            out.writeInt(values.length * 2 + 1600);
            for (boolean strict : new boolean[] {false, true}) {
                android.compat.Compatibility.setBehaviorChangeDelegate(new android.compat.Compatibility.BehaviorChangeDelegate() {
                    @Override public boolean isChangeEnabled(long id) { return id == 263076149L ? strict : true; }
                });
                for (String value : values) write(out, value, strict);
                var random = new java.util.Random(16);
                String[] pieces = {"", "0", "1", "f", "+1", "10000", "ffffffff", "7fffffffffffffff", "8000000000000000", "١", "Ｆ", " ", "g"};
                for (int i = 0; i < 800; i++) {
                    var parts = new String[5]; for (int j = 0; j < 5; j++) parts[j] = pieces[random.nextInt(pieces.length)];
                    write(out, String.join("-", parts) + (i % 7 == 0 ? "-" : ""), strict);
                }
            }
            java.nio.file.Files.write(new java.io.File(directory, "domain-uuid.original").toPath(), out.marshall());
        } finally { android.compat.Compatibility.clearBehaviorChangeDelegate(); out.recycle(); }
    }
    private static void write(android.os.Parcel out, String value, boolean strict) {
        out.writeBoolean(strict); out.writeString(value);
        try { String parsed = java.util.UUID.fromString(value).toString(); out.writeInt(0); out.writeString(parsed); }
        catch (IllegalArgumentException error) { out.writeInt(1); out.writeString(error.getMessage()); }
    }
    private DomainUuidOracle() {}
}
