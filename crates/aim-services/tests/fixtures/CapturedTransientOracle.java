import android.os.Parcel;
import dev.aim.server.PackageObjects;
import dev.aim.server.PackageTransientState;
import com.android.server.pm.CapturedPackageSetting;
import com.android.server.pm.PackageSetting;
import com.android.server.pm.pkg.PackageState;

/** Native-owned transient inputs restored through the pinned original setters/getters. */
public final class CapturedTransientOracle {
    public static void verify(java.io.File file, dev.aim.server.PackageSettingData metadata,
            dev.aim.server.PackageScanLease lease) throws Exception {
        var fresh = CapturedPackageSetting.from(metadata, 1, false);
        var original = (PackageState)fresh;
        if (original.isHiddenUntilInstalled() || original.isUpdatedSystemApp()
                || original.isApkInUpdatedApex() || original.getApexModuleName() != null) {
            throw new AssertionError("fresh original transient owner differs");
        }
        var captured = lease.getTransientState(metadata.getPackageName(), false);
        if (captured != lease.getTransientState(metadata.getPackageName(), false)
                || lease.getTransientState("missing", false) != null
                || lease.getTransientState(metadata.getPackageName(), true) != null) {
            throw new AssertionError("transient capture lease identity/scope differs");
        }
        for (int variant = 0; variant < 48; variant++) {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".transient-" + variant).toPath());
            Parcel in = Parcel.obtain();
            PackageTransientState state;
            try {
                in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
                state = PackageTransientState.CREATOR.createFromParcel(in);
                if (in.dataAvail() != 0) throw new AssertionError("trailing transient input");
            } finally { in.recycle(); }
            boolean factory = variant >= 24;
            String module = switch ((variant % 24) / 8) { case 0 -> null; case 1 -> ""; default -> "com.example.apex"; };
            if (state.isFactory() != factory || state.isHiddenUntilInstalled() != ((variant & 1) != 0)
                    || state.isUpdatedSystemApp() != ((variant & 2) != 0)
                    || state.isApkInUpdatedApex() != ((variant & 4) != 0)
                    || !java.util.Objects.equals(state.getApexModuleName(), module)) {
                throw new AssertionError("native transient input differs");
            }
            var setting = CapturedPackageSetting.from(metadata, 1, false);
            for (int wrong = 0; wrong < 3; wrong++) {
                setting.setAppId(metadata.appId + (wrong == 2 ? 1 : 0));
                try {
                    PackageObjects.restoreTransientState(setting, state, wrong == 0 ? 2 : 1,
                        wrong == 1 ? !factory : factory);
                    throw new AssertionError("wrong transient identity accepted");
                } catch (IllegalArgumentException expected) {}
                var rejected = (PackageState)setting;
                if (rejected.isHiddenUntilInstalled() || rejected.isUpdatedSystemApp()
                        || rejected.isApkInUpdatedApex() || rejected.getApexModuleName() != null) {
                    throw new AssertionError("rejected transient restore changed owner");
                }
            }
            setting.setAppId(metadata.appId);
            PackageObjects.restoreTransientState(setting, state, 1, factory);
            var restored = (PackageState)setting;
            if (restored.isHiddenUntilInstalled() != state.isHiddenUntilInstalled()
                    || restored.isUpdatedSystemApp() != state.isUpdatedSystemApp()
                    || restored.isApkInUpdatedApex() != state.isApkInUpdatedApex()
                    || !java.util.Objects.equals(restored.getApexModuleName(), module)) {
                throw new AssertionError("original transient getters differ");
            }
            Parcel out = Parcel.obtain();
            try {
                out.writeLong(1); out.writeString(restored.getPackageName()); out.writeInt(restored.getAppId());
                out.writeBoolean(factory); out.writeBoolean(restored.isHiddenUntilInstalled());
                out.writeBoolean(restored.isUpdatedSystemApp()); out.writeBoolean(restored.isApkInUpdatedApex());
                out.writeString(restored.getApexModuleName());
                if (!java.util.Arrays.equals(bytes, out.marshall())) throw new AssertionError("original transient owner bytes differ");
                out.setDataPosition(0); state.writeToParcel(out, 0);
                if (!java.util.Arrays.equals(bytes, out.marshall())) throw new AssertionError("transient DTO bytes differ");
            } finally { out.recycle(); }
            var copy = new PackageSetting(setting, false);
            setting.getPkgState().setHiddenUntilInstalled(!state.isHiddenUntilInstalled())
                .setUpdatedSystemApp(!state.isUpdatedSystemApp()).setApkInUpdatedApex(!state.isApkInUpdatedApex())
                .setApexModuleName("changed");
            var copied = (PackageState)copy;
            if (copied.isHiddenUntilInstalled() != state.isHiddenUntilInstalled()
                    || copied.isUpdatedSystemApp() != state.isUpdatedSystemApp()
                    || copied.isApkInUpdatedApex() != state.isApkInUpdatedApex()
                    || !java.util.Objects.equals(copied.getApexModuleName(), module)) {
                throw new AssertionError("original transient copy shares state");
            }
            // Original Parcel rounds unmarshalled storage to words. The lease
            // separately rejects byte arrays that end inside a word.
            for (int length = 0; length < bytes.length; length += 4) {
                Parcel truncated = Parcel.obtain();
                try {
                    truncated.unmarshall(bytes, 0, length); truncated.setDataPosition(0);
                    try { PackageTransientState.CREATOR.createFromParcel(truncated); throw new AssertionError("truncated transient accepted: variant=" + variant + " length=" + length + " full=" + bytes.length + " available=" + truncated.dataAvail()); }
                    catch (RuntimeException expected) {}
                } finally { truncated.recycle(); }
            }
            Parcel markers = Parcel.obtain();
            try {
                markers.unmarshall(bytes, 0, bytes.length); markers.setDataPosition(0);
                markers.readLong(); markers.readString(); markers.readInt();
                int first = markers.dataPosition();
                for (int index = 0; index < 4; index++) {
                    byte[] malformed = bytes.clone();
                    java.util.Arrays.fill(malformed, first + index * 4, first + index * 4 + 4, (byte) 0);
                    malformed[first + index * 4] = 2;
                    markers.unmarshall(malformed, 0, malformed.length); markers.setDataPosition(0);
                    try { PackageTransientState.CREATOR.createFromParcel(markers); throw new AssertionError("invalid transient boolean accepted"); }
                    catch (IllegalArgumentException expected) {}
                }
            } finally { markers.recycle(); }
        }
    }
}
