package dev.aim.server;

import android.os.Parcel;
import com.android.server.pm.PackageSetting;
import com.android.server.pm.SharedUserSetting;
import com.android.server.pm.pkg.SharedUserApi;

/** Distinct same-name members reconstructed through the production replica. */
public final class RetainedSharedUserOracle {
    public static void verify(java.io.File directory, PackageSetting old,
            PackageSetting adopted, SharedUserSetting group) throws Exception {
        verifyCollectedUid(directory, adopted);
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-shared-instance.record").toPath());
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var data = SharedUserData.CREATOR.createFromParcel(in);
            if (in.dataAvail() != 0 || data.getVersion() != 17 || data.getMembers().size() != 2)
                throw new AssertionError("shared instance envelope differs");
            var current = new PackageStateReplica(() -> new PackageSetting(adopted, true), java.util.Map.of(), 2);
            var members = new java.util.ArrayList<PackageStateReplica>();
            PackageStateReplica prior = null;
            for (var member : data.getMembers()) {
                if (member.retained() == null) members.add(current);
                else { prior = member.retained().newReplica(true); members.add(prior); }
            }
            if (prior == null || prior == current || prior.getPkg() != null
                    || prior.getAppId() != old.getAppId() || prior.getFlags() != old.getFlags()
                    || prior.getPrivateFlags() != old.getPrivateFlags()
                    || prior.getUserStateOrDefault(0).isInstalled() != old.readUserState(0).isInstalled()
                    || prior.getUserStates().size() != 1 || !prior.isInstallPermissionsFixed()
                    || prior.getHiddenApiEnforcementPolicy() != 2) throw new AssertionError("retained package replica differs");
            var replica = new SharedUserReplica(data, members);
            SharedUserApi original = (SharedUserApi)(Object)group.snapshot();
            if (replica.getPackageStates().size() != original.getPackageStates().size()
                    || replica.getPackageStates().size() != 2
                    || replica.getSeInfoTargetSdkVersion() != original.getSeInfoTargetSdkVersion()) {
                throw new AssertionError("same-name shared replica collapsed setting instances");
            }
            var states = replica.getPackageStates(); states.clear();
            if (replica.getPackageStates().size() != 2) throw new AssertionError("shared instance container escaped");
            var out = Parcel.obtain();
            try {
                data.writeToParcel(out, 0);
                if (!java.util.Arrays.equals(bytes, out.marshall())) throw new AssertionError("retained instance roundtrip differs");
            } finally { out.recycle(); }
        } finally { in.recycle(); }
    }
    private static void verifyCollectedUid(java.io.File directory, PackageSetting adopted) throws Exception {
        Parcel settingFrame = Parcel.obtain(); Parcel codeFrame = Parcel.obtain();
        try {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-shared-captured.setting").toPath());
            settingFrame.unmarshall(bytes, 0, bytes.length); settingFrame.setDataPosition(0);
            var data = PackageSettingData.read(settingFrame);
            bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-shared-captured.code").toPath());
            codeFrame.unmarshall(bytes, 0, bytes.length); codeFrame.setDataPosition(0);
            var code = PackageCode.CREATOR.createFromParcel(codeFrame);
            var restored = com.android.server.pm.CapturedPackageSetting.from(data, data.getVersion(), false);
            PackageObjects.restoreCollectedCode(restored, code, data.getVersion(), false);
            var restoredPkg = (com.android.internal.pm.parsing.pkg.PackageImpl)((com.android.server.pm.pkg.PackageStateInternal)(Object)restored).getPkg();
            var adoptedPkg = (com.android.internal.pm.parsing.pkg.PackageImpl)((com.android.server.pm.pkg.PackageStateInternal)(Object)adopted).getPkg();
            if (restored.getAppId() != adopted.getAppId() || restoredPkg.getUid() != adoptedPkg.getUid()
                    || !restoredPkg.isApex() || restored.getAppId() != 10000 || restoredPkg.getUid() != -1) {
                throw new AssertionError("shared APEX code/setting UID restoration differs");
            }
            restored.setAppId(10001);
            try {
                PackageObjects.restoreCollectedCode(restored, code, data.getVersion(), false);
                throw new AssertionError("foreign shared APEX setting UID accepted");
            } catch (IllegalArgumentException expected) {}
            if (settingFrame.dataAvail() != 0 || codeFrame.dataAvail() != 0)
                throw new AssertionError("APEX capture frame tail");
        } finally { settingFrame.recycle(); codeFrame.recycle(); }
    }

}
