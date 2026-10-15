package dev.aim.server;

import android.os.Parcel;
import com.android.server.pm.pkg.PackageState;
import com.android.server.pm.pkg.PackageStateInternal;

/** Original runtime owners captured alongside the package feed's immutable snapshot. */
public final class PackageRuntimeFeed {
    private PackageRuntimeFeed() {}

    public static byte[] capture(PackageState state) {
        if (!(state instanceof PackageStateInternal internal)) throw new IllegalArgumentException("missing internal package owner");
        Parcel out = Parcel.obtain();
        try {
            out.writeString(state.getPackageName()); out.writeInt(state.getAppId());
            out.writeString(internal.getPathString()); out.writeLong(state.getVersionCode());
            out.writeBoolean(state.getAndroidPackage() != null);
            var runtime = internal.getTransientState();
            out.writeString(runtime.getSeInfo()); out.writeString(runtime.getOverrideSeInfo());
            long[] usage = internal.getLastPackageUsageTime();
            if (usage == null || usage.length != 8) throw new IllegalArgumentException("invalid original usage owner");
            out.writeLongArray(usage);
            var files = state.getUsesLibraryFiles();
            out.writeInt(files.size());
            for (String file : files) out.writeString(file);
            PackageLibraryFeed.write(out, state.getSharedLibraryDependencies());
            out.writeBoolean(state.isHiddenUntilInstalled()); out.writeBoolean(state.isUpdatedSystemApp());
            out.writeBoolean(state.isApkInUpdatedApex()); out.writeString(state.getApexModuleName());
            return out.marshall();
        } finally { out.recycle(); }
    }
}
