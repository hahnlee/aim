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
    public static void notifyScanResults(byte[] bytes) {
        notifyScanResults(Objects.requireNonNull(ApexManager.getInstance()), bytes);
    }

    public static void notifyScanResults(ApexManager owner, byte[] bytes) {
        Objects.requireNonNull(owner);
        Objects.requireNonNull(bytes);
        if (bytes.length < 4) throw new IllegalArgumentException("missing APEX result count");
        if ((bytes.length & 3) != 0) throw new IllegalArgumentException("unaligned APEX scan results");
        Parcel in = Parcel.obtain();
        var results = new java.util.ArrayList<ApexManager.ScanResult>();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            int count = in.readInt();
            if (count < 0 || count > in.dataAvail() / 40) throw new IllegalArgumentException("invalid APEX result count");
            for (int i = 0; i < count; i++) {
                ApexInfo info = new ApexInfo();
                info.moduleName = in.readString();
                info.modulePath = Objects.requireNonNull(in.readString());
                info.preinstalledModulePath = Objects.requireNonNull(in.readString());
                info.versionCode = in.readLong();
                info.isFactory = readBoolean(in); info.isActive = readBoolean(in);
                info.activeApexChanged = readBoolean(in);
                byte[] cache = Objects.requireNonNull(in.createByteArray());
                int pastCount = in.readInt();
                if (pastCount < -1 || pastCount > in.dataAvail() / 8) throw new IllegalArgumentException("invalid APEX signer count");
                byte[][] certificates = pastCount == -1 ? null : new byte[pastCount][];
                int[] capabilities = pastCount == -1 ? null : new int[pastCount];
                for (int j = 0; j < pastCount; j++) {
                    certificates[j] = Objects.requireNonNull(in.createByteArray());
                    capabilities[j] = in.readInt();
                }
                var pkg = dev.aim.server.PackageObjects.fromCache(cache, certificates, capabilities);
                if (!pkg.isApex() || pkg.getUid() != -1 || !info.modulePath.equals(pkg.getPath())
                        || info.versionCode != pkg.getLongVersionCode()) {
                    throw new IllegalArgumentException("APEX result differs from completed code");
                }
                results.add(new ApexManager.ScanResult(info, pkg, Objects.requireNonNull(pkg.getPackageName())));
            }
            if (in.dataAvail() != 0) throw new IllegalArgumentException("trailing APEX scan results");
        } finally { in.recycle(); }
        owner.notifyScanResult(results);
    }

    private static boolean readBoolean(Parcel in) {
        int value = in.readInt();
        if (value != 0 && value != 1) throw new IllegalArgumentException("invalid APEX boolean");
        return value != 0;
    }
    private ApexBootFeed() {}
}
