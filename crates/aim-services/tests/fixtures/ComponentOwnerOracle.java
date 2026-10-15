// readComponentsLPr loop ported from pinned Android 16 Settings.java.
// Copyright (C) The Android Open Source Project, Apache License 2.0.
import android.util.ArraySet;
import android.util.Xml;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.DataOutputStream;
public final class ComponentOwnerOracle {
    public static void main(String[] args) throws Exception {
        int count = Integer.parseInt(args[1]);
        for (int i = 0; i < count; i++) {
            ArraySet<String> components = null;
            try (var input = new FileInputStream(args[0] + "/component-" + i + ".xml")) {
                var parser = Xml.resolvePullParser(input);
                while (parser.next() != 2) {}
                int outerDepth = parser.getDepth(), type;
                while ((type = parser.next()) != 1
                        && (type != 3 || parser.getDepth() > outerDepth)) {
                    if (type == 3 || type == 4) continue;
                    if (parser.getName().equals("item")) {
                        String name = parser.getAttributeValue(null, "name");
                        if (name != null) {
                            if (components == null) components = new ArraySet<>();
                            components.add(name);
                        }
                    }
                }
            }
            // PackageSetting.setUserState's ArraySet setter turns null into empty.
            try (var output = new DataOutputStream(new FileOutputStream(args[0] + "/component-" + i + ".original"))) {
                output.writeInt(components == null ? 0 : components.size());
                if (components != null) for (String name : components) {
                    byte[] bytes = name.getBytes(java.nio.charset.StandardCharsets.UTF_8);
                    output.writeInt(bytes.length); output.write(bytes);
                }
            }
        }
        System.out.println("COMPONENTS " + count);
    }
}
