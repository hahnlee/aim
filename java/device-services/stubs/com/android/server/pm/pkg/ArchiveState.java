// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.pkg;

import android.content.ComponentName;

import java.nio.file.Path;
import java.util.List;

public class ArchiveState {
    public ArchiveState(List<ArchiveActivityInfo> activityInfos, String installerTitle) { throw new RuntimeException("stub"); }
    public ArchiveState(List<ArchiveActivityInfo> activityInfos, String installerTitle, long time) { throw new RuntimeException("stub"); }
    public List<ArchiveActivityInfo> getActivityInfos() { throw new RuntimeException("stub"); }
    public String getInstallerTitle() { throw new RuntimeException("stub"); }
    public long getArchiveTimeMillis() { throw new RuntimeException("stub"); }

    public static final class ArchiveActivityInfo {
        public ArchiveActivityInfo(String title, ComponentName originalComponentName, Path iconBitmap, Path monochromeIconBitmap) { throw new RuntimeException("stub"); }
        public String getTitle() { throw new RuntimeException("stub"); }
        public ComponentName getOriginalComponentName() { throw new RuntimeException("stub"); }
        public Path getIconBitmap() { throw new RuntimeException("stub"); }
        public Path getMonochromeIconBitmap() { throw new RuntimeException("stub"); }
    }
}
