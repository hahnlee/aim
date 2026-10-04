package com.android.server.pm;

import android.apex.ApexInfo;
import android.os.Parcel;
import java.util.List;
import java.util.Objects;

/** Original APEX owner inputs before InitAppsHelper scans packages. */
public final class ApexBootFeed {
    public static byte[] capture() {
        var owner = Objects.requireNonNull(ApexManager.getInstance());
        return capture(owner.getAllApexInfos(), owner.getActiveApexInfos());
    }

    public static byte[] capture(ApexInfo[] all, List<ApexManager.ActiveApexInfo> active) {
        Objects.requireNonNull(active);
        Parcel out = Parcel.obtain();
        try {
            out.writeInt(all == null ? -1 : all.length);
            if (all != null) for (var info : all) {
                Objects.requireNonNull(info);
                out.writeString(info.moduleName);
                out.writeString(Objects.requireNonNull(info.modulePath));
                out.writeString(Objects.requireNonNull(info.preinstalledModulePath));
                out.writeLong(info.versionCode);
                out.writeBoolean(info.isFactory); out.writeBoolean(info.isActive);
                out.writeBoolean(info.activeApexChanged);
            }
            out.writeInt(active.size());
            for (var info : active) {
                Objects.requireNonNull(info);
                out.writeString(info.apexModuleName);
                out.writeString(info.apexDirectory.getAbsolutePath());
                out.writeString(info.preInstalledApexPath.getAbsolutePath());
                out.writeBoolean(info.isFactory);
                out.writeString(info.apexFile.getAbsolutePath());
                out.writeBoolean(info.activeApexChanged);
            }
            return out.marshall();
        } finally { out.recycle(); }
    }
    private ApexBootFeed() {}
}
