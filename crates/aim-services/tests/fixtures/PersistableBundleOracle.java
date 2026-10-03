import android.os.PersistableBundle;
import android.os.Parcel;
import android.util.Xml;
import com.android.server.pm.pkg.SuspendParams;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public final class PersistableBundleOracle {
    private static void text(DataOutputStream out, String value) throws Exception {
        if (value == null) { out.writeInt(-1); return; }
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        out.writeInt(bytes.length);
        out.write(bytes);
    }
    private static void bundle(DataOutputStream out, PersistableBundle bundle) throws Exception {
        if (bundle == null) { out.writeInt(-1); return; }
        var keys = new ArrayList<String>(bundle.keySet());
        keys.sort(Comparator.nullsFirst(Comparator.naturalOrder()));
        out.writeInt(keys.size());
        for (String key : keys) {
            text(out, key);
            Object value = bundle.get(key);
            if (value == null) { out.writeByte(0); }
            else if (value instanceof Integer v) { out.writeByte(1); out.writeInt(v); }
            else if (value instanceof Long v) { out.writeByte(2); out.writeLong(v); }
            else if (value instanceof Double v) { out.writeByte(3); out.writeLong(Double.doubleToRawLongBits(v)); }
            else if (value instanceof Boolean v) { out.writeByte(4); out.writeBoolean(v); }
            else if (value instanceof String v) { out.writeByte(5); text(out, v); }
            else if (value instanceof int[] v) { out.writeByte(6); out.writeInt(v.length); for (int x : v) out.writeInt(x); }
            else if (value instanceof long[] v) { out.writeByte(7); out.writeInt(v.length); for (long x : v) out.writeLong(x); }
            else if (value instanceof double[] v) { out.writeByte(8); out.writeInt(v.length); for (double x : v) out.writeLong(Double.doubleToRawLongBits(x)); }
            else if (value instanceof boolean[] v) { out.writeByte(9); out.writeInt(v.length); for (boolean x : v) out.writeBoolean(x); }
            else if (value instanceof String[] v) { out.writeByte(10); out.writeInt(v.length); for (String x : v) text(out, x); }
            else if (value instanceof PersistableBundle v) { out.writeByte(11); bundle(out, v); }
            else throw new AssertionError("unexpected persistable type: " + value);
        }
    }
    public static void main(String[] args) throws Exception {
        int count = Integer.parseInt(args[1]);
        for (int i = 0; i < count; i++) {
            var bytes = new ByteArrayOutputStream();
            var out = new DataOutputStream(bytes);
            try (var input = new FileInputStream(args[0] + "/extras-" + i + ".xml")) {
                var parser = Xml.resolvePullParser(input);
                while (parser.next() != 2) {}
                if ("float-value".equals(parser.getName())) {
                    float value = parser.getAttributeFloat(null, "value");
                    out.writeByte(0);
                    out.writeInt(Float.floatToIntBits(value));
                } else if ("suspend-params".equals(parser.getName())) {
                    var params = SuspendParams.restoreFromXml(parser);
                    out.writeByte(0);
                    out.writeBoolean(params.isQuarantined());
                    var dialog = params.getDialogInfo();
                    out.writeBoolean(dialog != null);
                    if (dialog != null) {
                        out.writeInt(dialog.getIconResId());
                        out.writeInt(dialog.getTitleResId()); text(out, dialog.getTitle());
                        out.writeInt(dialog.getDialogMessageResId()); text(out, dialog.getDialogMessage());
                        out.writeInt(dialog.getNeutralButtonTextResId()); text(out, dialog.getNeutralButtonText());
                        out.writeInt(dialog.getNeutralButtonAction());
                    }
                    bundle(out, params.getAppExtras());
                    bundle(out, params.getLauncherExtras());
                } else {
                    var bundle = PersistableBundle.restoreFromXml(parser);
                    out.writeByte(0);
                    bundle(out, bundle);
                }
            } catch (Exception error) {
                bytes.reset();
                out.writeByte(error.getClass().getName().equals("org.xmlpull.v1.XmlPullParserException") ? 1 : 2);
            }
            try (var file = new FileOutputStream(args[0] + "/extras-" + i + ".original")) {
                file.write(bytes.toByteArray());
            }
        }
        System.out.println("EXTRAS " + count);
        int parcels = Integer.parseInt(args[2]);
        for (int i = 0; i < parcels; i++) {
            byte[] nativeBytes = java.nio.file.Files.readAllBytes(
                    java.nio.file.Path.of(args[0], "parcel-" + i + ".native"));
            Parcel input = Parcel.obtain();
            Parcel output = Parcel.obtain();
            try {
                input.unmarshall(nativeBytes, 0, nativeBytes.length);
                input.setDataPosition(0);
                PersistableBundle value = input.readPersistableBundle();
                var bytes = new ByteArrayOutputStream();
                bundle(new DataOutputStream(bytes), value);
                if (input.readInt() != 0x11ddee55 || input.dataAvail() != 0) {
                    throw new AssertionError("native bundle consumed the wrong byte range");
                }
                java.nio.file.Files.write(java.nio.file.Path.of(args[0], "parcel-" + i + ".semantic"), bytes.toByteArray());
                output.writePersistableBundle(value);
                output.writeInt(0x11ddee55);
                java.nio.file.Files.write(java.nio.file.Path.of(args[0], "parcel-" + i + ".original"), output.marshall());
            } finally {
                input.recycle(); output.recycle();
            }
        }
        System.out.println("BUNDLES " + parcels);
    }
}
