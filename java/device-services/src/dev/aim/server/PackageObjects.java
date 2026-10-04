package dev.aim.server;

import android.content.pm.Signature;
import android.content.pm.SigningDetails;
import android.util.ArraySet;
import com.android.internal.pm.parsing.pkg.PackageImpl;
import com.android.server.pm.parsing.PackageCacher;
import java.util.Arrays;
import java.util.Objects;

/** Original package objects decoded from native collected code before publication. */
public final class PackageObjects {
    private PackageObjects() {}

    public static PackageImpl fromSnapshot(PackageCode code, long version, String name) {
        if (code.getVersion() != version || !code.getPackageName().equals(name)) {
            throw new IllegalArgumentException("package code capture mismatch");
        }
        PackageImpl pkg = fromCache(code.getCache(), code.getCertificates(), code.getCapabilities());
        if (!name.equals(pkg.getPackageName())) {
            throw new IllegalArgumentException("package code name mismatch");
        }
        return pkg;
    }

    public static void restoreCollectedCode(com.android.server.pm.PackageSetting setting,
            PackageCode code, long version, boolean factory) {
        PackageImpl pkg = fromSnapshot(code, version, setting.getPackageName());
        var state = (com.android.server.pm.pkg.PackageState)setting;
        boolean uidMatches = pkg.isApex()
            ? pkg.getUid() == -1 && (factory
                || setting.getAppId() == (setting.hasSharedUser() ? state.getSharedUserAppId() : -1))
            : factory || pkg.getUid() == setting.getAppId();
        if (!uidMatches
                || !Objects.equals(state.getPath(), new java.io.File(pkg.getPath()))
                || state.getVersionCode() != pkg.getLongVersionCode()) {
            throw new IllegalArgumentException("package code setting mismatch");
        }
        setting.setPkg(pkg);
    }

    public static void restoreSavedSigning(com.android.server.pm.PackageSetting setting,
            PackageSigningState state, long version, boolean factory) {
        if (state.getVersion() != version || !state.getPackageName().equals(setting.getPackageName())
                || state.getAppId() != setting.getAppId() || state.isDisabled() != factory
                || (state.getSharedGroupName() != null) != setting.hasSharedUser()
                || (setting.hasSharedUser() && ((com.android.server.pm.pkg.PackageState)setting)
                        .getSharedUserAppId() != state.getSharedAppId())) {
            throw new IllegalArgumentException("saved signing capture mismatch");
        }
        setting.setSigningDetails(state.getPackageSigningDetails());
    }

    public static void restoreUsage(com.android.server.pm.PackageSetting setting,
            PackageUsageState usage, long version) {
        if (usage.getVersion() != version || !usage.getPackageName().equals(setting.getPackageName())) {
            throw new IllegalArgumentException("package usage capture mismatch");
        }
        long[] times = usage.getLastPackageUsageTimeInMills();
        for (int reason = 0; reason < times.length; reason++) {
            setting.getPkgState().setLastPackageUsageTimeInMills(reason, times[reason]);
        }
    }

    public static void restoreLibraries(com.android.server.pm.PackageSetting setting,
            PackageLibraryState state, long version) {
        if (state.getVersion() != version || !state.getPackageName().equals(setting.getPackageName())
                || state.getAppId() != setting.getAppId()) throw new IllegalArgumentException("library capture mismatch");
        var infos = state.getLibraries();
        var files = state.getFiles();
        setting.getPkgState().setUsesLibraryInfos(infos);
        setting.getPkgState().setUsesLibraryFiles(files);
    }

    public static void restoreTransientState(com.android.server.pm.PackageSetting setting,
            PackageTransientState state, long version, boolean factory) {
        if (state.getVersion() != version || !state.getPackageName().equals(setting.getPackageName())
                || state.getAppId() != setting.getAppId() || state.isFactory() != factory) {
            throw new IllegalArgumentException("transient setting capture mismatch");
        }
        setting.getPkgState().setHiddenUntilInstalled(state.isHiddenUntilInstalled())
            .setUpdatedSystemApp(state.isUpdatedSystemApp()).setApkInUpdatedApex(state.isApkInUpdatedApex())
            .setApexModuleName(state.getApexModuleName());
    }

    /** Boot shared-user fixups set an override; ordinary scan labels set the base. */
    public static void restoreBootSeInfo(com.android.server.pm.PackageSetting setting,
            PackageSeInfoState state, long version) {
        if (state.getVersion() != version || !state.getPackageName().equals(setting.getPackageName())) {
            throw new IllegalArgumentException("package seInfo capture mismatch");
        }
        if (state.isOverride()) setting.getPkgState().setOverrideSeInfo(state.getLabel());
        else setting.getPkgState().setSeInfo(state.getLabel());
    }

    /** Complete replica restoration requires the scan's base as well as its override. */
    public static void restoreSeInfo(com.android.server.pm.PackageSetting setting,
            PackageSeInfoState state, long version) {
        if (state.getVersion() != version || !state.getPackageName().equals(setting.getPackageName())) {
            throw new IllegalArgumentException("package seInfo capture mismatch");
        }
        if (state.getBaseLabel() == null) throw new IllegalStateException("seInfo base is not assigned");
        setting.getPkgState().setSeInfo(state.getBaseLabel());
        setting.getPkgState().setOverrideSeInfo(state.getOverrideLabel());
    }

    public static void restoreRuntime(com.android.server.pm.PackageSetting setting,
            PackageRuntimeState state, long version, boolean factory) {
        if (state.getVersion() != version || !state.getPackageName().equals(setting.getPackageName())
                || state.getAppId() != setting.getAppId() || state.isFactory() != factory) {
            throw new IllegalArgumentException("runtime capture mismatch");
        }
        setting.getPkgState().setSeInfo(state.getSeInfo()).setOverrideSeInfo(state.getOverrideSeInfo());
        long[] usage = state.getUsage();
        for (int reason = 0; reason < usage.length; reason++) {
            setting.getPkgState().setLastPackageUsageTimeInMills(reason, usage[reason]);
        }
        restoreLibraries(setting, state.getLibraries(), version);
    }

    public static PackageImpl fromCache(byte[] cache, byte[][] pastCertificates, int[] capabilities) {
        PackageImpl pkg = (PackageImpl) PackageCacher.fromCacheEntryStatic(cache);
        pkg.setSigningDetails(restoreSigning(pkg.getSigningDetails(), pastCertificates, capabilities));
        return pkg;
    }

    // Signature's Parcel format omits flags. Bind the separate flags to certificate bytes
    // and copy every mutable array/signature, leaving the decoded object untouched.
    public static SigningDetails restoreSigning(SigningDetails cached,
            byte[][] pastCertificates, int[] capabilities) {
        Objects.requireNonNull(cached, "cached signing");
        Signature[] past = cached.getPastSigningCertificates();
        if ((past == null) != (pastCertificates == null)
                || (past == null) != (capabilities == null)
                || (past != null && (past.length != pastCertificates.length
                    || past.length != capabilities.length))) {
            throw new IllegalArgumentException("signing lineage metadata mismatch");
        }
        if (cached == SigningDetails.UNKNOWN) return cached;
        Signature[] restoredPast = copy(past);
        if (past != null) {
            for (int i = 0; i < past.length; i++) {
                if (pastCertificates[i] == null
                        || !Arrays.equals(past[i].toByteArray(), pastCertificates[i])) {
                    throw new IllegalArgumentException("signing lineage certificate mismatch");
                }
                restoredPast[i].setFlags(capabilities[i]);
            }
        }
        ArraySet<java.security.PublicKey> keys = null;
        if (cached.getPublicKeys() != null) {
            keys = new ArraySet<>();
            keys.addAll(cached.getPublicKeys());
        }
        return new SigningDetails(copy(cached.getSignatures()),
            cached.getSignatureSchemeVersion(), keys, restoredPast);
    }

    private static Signature[] copy(Signature[] values) {
        if (values == null) return null;
        Signature[] result = new Signature[values.length];
        for (int i = 0; i < values.length; i++) {
            result[i] = new Signature(values[i]);
        }
        return result;
    }
}
