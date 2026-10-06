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
            var bundles = android.os.Parcel.obtain();
            try {
                bundles.writeInt(10);
                for (int mode = 0; mode < 10; mode++) {
                    var bundle = new android.os.Bundle();
                    if (mode == 1) bundle.putString("x.example", "wrong type");
                    if (mode == 2) bundle.putParcelableArrayList("x.example", null);
                    if (mode >= 3 && mode <= 5) {
                        var groups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>();
                        if (mode == 4) groups.add(null);
                        if (mode == 5) {var group = new android.content.UriRelativeFilterGroupParcel(); group.action = 99; group.filters = new java.util.ArrayList<>(); var filter = new android.content.UriRelativeFilterParcel(); filter.filter = null; group.filters.add(filter); groups.add(group);}
                        bundle.putParcelableArrayList("x.example", groups);
                    }
                    if (mode >= 6) {
                        bundle = styledBundle("x.example", mode);
                    }
                    var wire = android.os.Parcel.obtain();
                    try {
                        bundle.writeToParcel(wire, 0); bundles.writeInt(mode); bundles.writeByteArray(wire.marshall());
                        if (mode >= 6 && bundle.getParcelableArrayList("x.example", android.content.UriRelativeFilterGroupParcel.class) != null) throw new AssertionError("styled URI value accepted as groups");
                    }
                    finally {wire.recycle();}
                }
                java.nio.file.Files.write(new java.io.File(directory, "uri-input-bundles.original").toPath(), bundles.marshall());
            } finally {bundles.recycle();}
            out.recycle(); out = android.os.Parcel.obtain();
            out.writeInt(3);
            for (int mode = 0; mode < 3; mode++) {
                var parcels = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>();
                var group = new android.content.UriRelativeFilterGroupParcel();
                if (mode == 0) parcels.add(null);
                else {
                    if (mode == 2) {group.filters = new java.util.ArrayList<>(); group.filters.add(null);}
                    parcels.add(group);
                }
                try {android.content.UriRelativeFilterGroup.parcelsToGroups(parcels); throw new AssertionError("null URI conversion accepted");}
                catch (NullPointerException error) {
                    out.writeInt(mode); out.writeString(error.getMessage());
                }
            }
            java.nio.file.Files.write(new java.io.File(directory, "uri-conversion-errors.original").toPath(), out.marshall());
            out.recycle(); out = android.os.Parcel.obtain();
            String[] uris = {"https://x/path?q=1#frag", "https://x", "mailto:x", "", "https://x?;", "#"};
            out.writeInt(4 * 4 * 7 * uris.length);
            for (String value : new String[] {null, "", "/path", "["}) for (int part : new int[] {-1, 0, 1, 2})
                for (int pattern : new int[] {-1, 0, 1, 2, 3, 4, 99}) for (String uri : uris) {
                    var parcel = new android.content.UriRelativeFilterParcel(); parcel.uriPart = part; parcel.patternType = pattern; parcel.filter = value;
                    out.writeInt(part); out.writeInt(pattern); out.writeString(value); out.writeString(uri);
                    int result;
                    try { result = new android.content.UriRelativeFilter(parcel).matchData(android.net.Uri.parse(uri)) ? 1 : 0; }
                    catch (NullPointerException error) { result = -1; }
                    catch (IllegalArgumentException error) { result = -2; }
                    catch (IndexOutOfBoundsException error) { result = -3; }
                    out.writeInt(result);
                }
            java.nio.file.Files.write(new java.io.File(directory, "uri-null-match.original").toPath(), out.marshall());
        } finally { out.recycle(); }
    }
    static android.os.Bundle styledBundle(String key, int mode) {
        var body = android.os.Parcel.obtain(); var wire = android.os.Parcel.obtain();
        try {
            body.writeInt(1); body.writeString(key); body.writeInt(mode == 7 ? 24 : 10);
            if (mode == 7) body.writeInt(2);
            body.writeInt(mode == 9 ? 2 : 0); body.writeString8(mode == 8 ? null : "styled");
            if (mode != 8) {
                body.writeInt(2); body.writeInt(0xff123456); // ForegroundColorSpan
                body.writeInt(0); body.writeInt(6); body.writeInt(33);
                body.writeInt(0);
            }
            if (mode == 7) {body.writeInt(1); body.writeString8(null);}
            byte[] bytes = body.marshall();
            wire.writeInt(bytes.length); wire.writeInt(0x4c444e42);
            wire.appendFrom(body, 0, bytes.length); wire.writeBoolean(false); wire.setDataPosition(0);
            var result = android.os.Bundle.CREATOR.createFromParcel(wire);
            if (wire.dataAvail() != 0) throw new AssertionError("styled Bundle framing differs");
            return result;
        } finally {body.recycle(); wire.recycle();}
    }
    private UriDtoOracle() {}
}
