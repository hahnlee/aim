package com.android.server.pm;

import android.os.Parcel;
import android.content.pm.overlay.OverlayPaths;
import com.android.server.pm.pkg.PackageStateInternal;
import com.android.server.pm.pkg.PackageUserStateImpl;
import java.util.Map;

/** Complete writable surface of the original PackageStateMutator, protocol 1. */
public final class NativePackageMutationRecord {
    private NativePackageMutationRecord() {}
    public static byte[] encode(long version, Map<String, PackageSetting> active,
            Map<String, PackageSetting> disabled) {
        Parcel parcel = Parcel.obtain();
        try {
            parcel.writeInt(2); parcel.writeLong(version);
            write(parcel, active, false); write(parcel, disabled, true);
            return parcel.marshall();
        } finally { parcel.recycle(); }
    }
    private static void paths(Parcel out, OverlayPaths paths) {
        out.writeBoolean(paths != null);
        if (paths == null) return;
        out.writeStringList(paths.getResourceDirs());
        out.writeStringList(paths.getOverlayPaths());
    }
    private static void write(Parcel out, Map<String, PackageSetting> settings, boolean factory) {
        out.writeInt(settings.size());
        for (var entry : settings.entrySet()) {
            PackageSetting setting = entry.getValue();
            PackageStateInternal state = (PackageStateInternal) setting;
            if (!entry.getKey().equals(state.getPackageName()))
                throw new IllegalArgumentException("mutation package identity differs");
            out.writeString(entry.getKey()); out.writeBoolean(factory); out.writeInt(state.getAppId());
            out.writeInt(state.getPrivateFlags()); out.writeInt(state.getCategoryOverride());
            out.writeInt(setting.getPageSizeAppCompatFlags()); out.writeBoolean(state.isUpdateAvailable());
            out.writeFloat(state.getLoadingProgress()); out.writeLong(state.getLoadingCompletedTime());
            out.writeBoolean(state.isHiddenUntilInstalled());
            out.writeString(state.getTransientState().getOverrideSeInfo());
            out.writeLongArray(state.getTransientState().getLastPackageUsageTimeInMills());
            var source = state.getInstallSource();
            out.writeString(source.mInstallerPackageName); out.writeInt(source.mInstallerPackageUid);
            out.writeString(source.mUpdateOwnerPackageName);
            var mime = setting.getMimeGroups();
            out.writeInt(mime == null ? -1 : mime.size());
            if (mime != null) for (var group : mime.entrySet()) {
                out.writeString(group.getKey());
                out.writeStringList(new java.util.ArrayList<>(group.getValue()));
            }
            var users = state.getUserStates(); out.writeInt(users.size());
            for (int i = 0; i < users.size(); i++) {
                out.writeInt(users.keyAt(i));
                var user = users.valueAt(i);
                out.writeBoolean(user.isInstalled()); out.writeInt(user.getUninstallReason());
                out.writeInt(user.getDistractionFlags()); out.writeBoolean(user.isHidden());
                out.writeBoolean(user.isStopped()); out.writeBoolean(user.isNotLaunched());
                out.writeString(user.getHarmfulAppWarning()); out.writeString(user.getSplashScreenTheme());
                out.writeInt(user.getMinAspectRatio());
                paths(out, user.getOverlayPaths());
                var libraries = user.getSharedLibraryOverlayPaths();
                out.writeInt(libraries == null ? -1 : libraries.size());
                if (libraries != null) for (var library : libraries.entrySet()) {
                    out.writeString(library.getKey()); paths(out, library.getValue());
                }
                var suspensions = user.getSuspendParams();
                out.writeInt(suspensions == null ? -1 : suspensions.size());
                if (suspensions != null) for (int j = 0; j < suspensions.size(); j++) {
                    var owner = suspensions.keyAt(j); var params = suspensions.valueAt(j);
                    out.writeString(owner.packageName); out.writeInt(owner.userId);
                    out.writeBoolean(params != null);
                    if (params != null) {
                        out.writeTypedObject((android.os.Parcelable) (Object) params.getDialogInfo(), 0);
                        out.writeTypedObject(params.getAppExtras(), 0);
                        out.writeTypedObject(params.getLauncherExtras(), 0);
                        out.writeBoolean(params.isQuarantined());
                    }
                }
                var overrides=((PackageUserStateImpl)(Object)user).mComponentLabelIconOverrideMap;
                out.writeInt(overrides==null?-1:overrides.size());
                if(overrides!=null)for(int j=0;j<overrides.size();j++) {
                    var component=overrides.keyAt(j);var value=overrides.valueAt(j);
                    out.writeString(component.getPackageName());out.writeString(component.getClassName());
                    out.writeString(value.first);out.writeBoolean(value.second!=null);
                    if(value.second!=null)out.writeInt(value.second);
                }
            }
        }
    }
}
