/** Reads and writes original sized AIDL URI DTOs, including nullable fields. */
final class UriDtoOracle {
    static void verify(java.io.File directory) throws Exception {
        var out = android.os.Parcel.obtain();
        try {
            out.writeInt(68);
            for (String value : new String[] {null, "", "a", "😀"}) for (int part : new int[] {-1, 0, 2, 99}) for (int pattern : new int[] {-1, 0, 3, 99}) {
                var filter = new android.content.UriRelativeFilterParcel(); filter.uriPart = part; filter.patternType = pattern; filter.filter = value;
                var wire = android.os.Parcel.obtain();
                try {
                    filter.writeToParcel(wire, 0); byte[] bytes = wire.marshall();
                    wire.setDataPosition(0); var read = android.content.UriRelativeFilterParcel.CREATOR.createFromParcel(wire);
                    if (read.uriPart != part || read.patternType != pattern || !java.util.Objects.equals(read.filter, value) || wire.dataAvail() != 0) throw new AssertionError("original filter DTO differs");
                    out.writeInt(0); out.writeInt(part); out.writeInt(pattern); out.writeString(value); out.writeByteArray(bytes);
                } finally { wire.recycle(); }
            }
            for (int mode = 0; mode < 4; mode++) {
                var group = new android.content.UriRelativeFilterGroupParcel(); group.action = mode - 1;
                if (mode != 0) group.filters = new java.util.ArrayList<>();
                if (mode >= 2) group.filters.add(null);
                if (mode == 3) { var filter = new android.content.UriRelativeFilterParcel(); filter.uriPart = 2; filter.patternType = 99; filter.filter = null; group.filters.add(filter); }
                var wire = android.os.Parcel.obtain();
                try {
                    group.writeToParcel(wire, 0); byte[] bytes = wire.marshall(); wire.setDataPosition(0);
                    var read = android.content.UriRelativeFilterGroupParcel.CREATOR.createFromParcel(wire);
                    if (read.action != group.action || (read.filters == null) != (group.filters == null) || wire.dataAvail() != 0) throw new AssertionError("original group DTO differs");
                    out.writeInt(1); out.writeInt(mode); out.writeByteArray(bytes);
                } finally { wire.recycle(); }
            }
            java.nio.file.Files.write(new java.io.File(directory, "uri-dto.original").toPath(), out.marshall());
        } finally { out.recycle(); }
    }
    private UriDtoOracle() {}
}
