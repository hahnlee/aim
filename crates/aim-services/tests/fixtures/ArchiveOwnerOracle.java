// Archive reader loops ported from pinned Android 16 Settings.java.
// Copyright (C) The Android Open Source Project, Apache License 2.0.
import android.content.ComponentName;
import android.util.Xml;
import com.android.server.pm.pkg.ArchiveState;
import java.io.DataOutputStream;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.nio.file.Path;
import java.util.ArrayList;

public final class ArchiveOwnerOracle {
    private static void text(DataOutputStream output, String value) throws Exception {
        if (value == null) { output.writeInt(-1); return; }
        byte[] bytes = value.getBytes(java.nio.charset.StandardCharsets.UTF_8);
        output.writeInt(bytes.length); output.write(bytes);
    }

    public static void main(String[] args) {
        try { check(args); } catch (Throwable error) {
            error.printStackTrace(); System.exit(1);
        }
    }

    private static void check(String[] args) throws Exception {
        int count = Integer.parseInt(args[1]);
        System.err.println("archive-oracle phase=start count=" + count);
        for (int i = 0; i < count; i++) {
            System.err.println("archive-oracle case=" + i + " phase=input");
            ArchiveState archive = null;
            boolean invalid = false;
            try (var input = new FileInputStream(args[0] + "/archive-" + i + ".xml")) {
                var parser = Xml.resolvePullParser(input);
                while (parser.next() != 2) {}
                String installer = parser.getAttributeValue(null, "installer-title");
                long time = parser.getAttributeLongHex(null, "archive-time", 0);
                var activities = new ArrayList<ArchiveState.ArchiveActivityInfo>();
                int outerDepth = parser.getDepth(), type;
                while ((type = parser.next()) != 1
                        && (type != 3 || parser.getDepth() > outerDepth)) {
                    if (type == 3 || type == 4) continue;
                    if (!parser.getName().equals("archive-activity-info")) continue;
                    String title = parser.getAttributeValue(null, "activity-title");
                    String name = parser.getAttributeValue(null, "original-component-name");
                    String icon = parser.getAttributeValue(null, "icon-path");
                    Path iconPath = icon == null ? null : Path.of(icon);
                    String mono = parser.getAttributeValue(null, "monochrome-icon-path");
                    Path monoPath = mono == null ? null : Path.of(mono);
                    if (title == null || name == null || iconPath == null) continue;
                    ComponentName component = ComponentName.unflattenFromString(name);
                    if (component == null) continue;
                    activities.add(new ArchiveState.ArchiveActivityInfo(title, component, iconPath, monoPath));
                }
                archive = installer == null || activities.isEmpty() ? null
                        : new ArchiveState(activities, installer, time);
            }
            catch (IllegalStateException error) { invalid = true; }
            System.err.println("archive-oracle case=" + i + " phase=parsed invalid=" + invalid);
            try (var output = new DataOutputStream(new FileOutputStream(args[0] + "/archive-" + i + ".original"))) {
                if (invalid) { output.writeByte(2); continue; }
                output.writeBoolean(archive != null);
                if (archive == null) continue;
                text(output, archive.getInstallerTitle());
                output.writeLong(archive.getArchiveTimeMillis());
                output.writeInt(archive.getActivityInfos().size());
                for (var activity : archive.getActivityInfos()) {
                    text(output, activity.getTitle());
                    text(output, activity.getOriginalComponentName().flattenToString());
                    text(output, activity.getIconBitmap().toString());
                    text(output, activity.getMonochromeIconBitmap() == null ? null
                            : activity.getMonochromeIconBitmap().toString());
                }
            } finally {
                System.err.println("archive-oracle case=" + i + " phase=output-scope-ended");
            }
        }
        System.err.println("archive-oracle phase=before-final-stdout count=" + count);
        System.out.println("ARCHIVES " + count);
        System.err.println("archive-oracle phase=after-final-stdout count=" + count);
    }
}
