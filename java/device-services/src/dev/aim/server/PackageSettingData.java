package dev.aim.server;

import android.os.Parcel;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;

/** Immutable captured scalar PackageSetting inputs; full facade assembly is separate. */
public final class PackageSettingData {
    private final long version;
    private final String name;
    private final boolean factory;
    private final byte[] restrictUpdateHash;
    private final List<String> oldPaths;
    public final InstallSourceData installSource;
    public final KeySetData keySets;
    public final String realName;
    public final String path;
    public final String legacyNativeLibraryPath;
    public final String primaryCpuAbiRaw;
    public final String secondaryCpuAbiRaw;
    public final String cpuAbiOverride;
    public final int flags;
    public final int privateFlags;
    public final long lastModifiedTime;
    public final long lastUpdateTime;
    public final long legacyFirstInstallTime;
    public final long versionCode;
    public final int targetSdkVersion;
    public final int appId;
    public final boolean sharedUser;
    public final boolean sdkLibrary;
    public final String volumeUuid;
    public final int categoryOverride;
    public final boolean updateAvailable;
    public final boolean forceQueryable;
    public final boolean pendingRestore;
    public final boolean debuggable;
    public final boolean scannedAsStoppedSystemApp;
    public final int baseRevisionCode;
    public final int pageSizeAppCompatFlags;
    public final float loadingProgress;
    public final long loadingCompletedTime;
    public final String domainSetId;
    public final String appMetadataFilePath;
    public final int appMetadataSource;

    private PackageSettingData(Parcel in) {
        version = in.readLong(); name = Objects.requireNonNull(in.readString()); factory = in.readBoolean();
        realName = in.readString();
        path = Objects.requireNonNull(in.readString());
        legacyNativeLibraryPath = in.readString();
        primaryCpuAbiRaw = in.readString();
        secondaryCpuAbiRaw = in.readString();
        cpuAbiOverride = in.readString();
        flags = in.readInt();
        privateFlags = in.readInt();
        lastModifiedTime = in.readLong();
        lastUpdateTime = in.readLong();
        legacyFirstInstallTime = in.readLong();
        versionCode = in.readLong();
        targetSdkVersion = in.readInt();
        appId = in.readInt();
        sharedUser = in.readBoolean();
        sdkLibrary = in.readBoolean();
        volumeUuid = in.readString();
        categoryOverride = in.readInt();
        updateAvailable = in.readBoolean();
        forceQueryable = in.readBoolean();
        pendingRestore = in.readBoolean();
        debuggable = in.readBoolean();
        scannedAsStoppedSystemApp = in.readBoolean();
        baseRevisionCode = in.readInt();
        pageSizeAppCompatFlags = in.readInt();
        loadingProgress = in.readFloat();
        loadingCompletedTime = in.readLong();
        domainSetId = in.readString();
        appMetadataFilePath = in.readString();
        appMetadataSource = in.readInt();
        restrictUpdateHash = in.createByteArray();
        int count = in.readInt();
        if (count < -1 || count > in.dataAvail() / 4) throw new IllegalArgumentException("invalid old paths count");
        if (count == -1) oldPaths = null;
        else {
            var paths = new ArrayList<String>(count);
            for (int i = 0; i < count; i++) paths.add(in.readString());
            oldPaths = Collections.unmodifiableList(paths);
        }
        installSource = new InstallSourceData(in);
        keySets = new KeySetData(in);
    }
    public static PackageSettingData read(Parcel in) { return new PackageSettingData(in); }
    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public boolean isFactory() { return factory; }
    public boolean isLoading() { return Math.abs(1f - loadingProgress) >= .00000001f; }
    public byte[] getRestrictUpdateHash() { return restrictUpdateHash == null ? null : restrictUpdateHash.clone(); }
    public List<String> getOldPaths() { return oldPaths; }
    public static final class InstallSourceData {
        public final String initiatingPackage, originatingPackage, installerPackage, updateOwner, attributionTag;
        public final int installerUid, packageSource;
        public final boolean orphaned, initiatingUninstalled;
        private final PackageSigningState.Signing initiatingSigning;
        private InstallSourceData(Parcel in) {
            initiatingPackage = in.readString(); originatingPackage = in.readString(); installerPackage = in.readString();
            installerUid = in.readInt(); updateOwner = in.readString(); attributionTag = in.readString();
            packageSource = in.readInt(); orphaned = in.readBoolean(); initiatingUninstalled = in.readBoolean();
            initiatingSigning = PackageSigningState.Signing.read(in);
            if (initiatingPackage == null && initiatingSigning != null) throw new IllegalArgumentException("signing owner has no initiating package");
            getInitiatingSigningDetails();
        }
        public android.content.pm.SigningDetails getInitiatingSigningDetails() {
            return initiatingSigning == null ? null : PackageSigningState.Signing.details(initiatingSigning);
        }
    }
    public com.android.server.pm.InstallSource getInstallSource() {
        return com.android.server.pm.CapturedInstallSource.from(installSource);
    }

    public static final class KeySetData {
        public final long properSigningKeySet;
        private final long[] upgrades;
        public record Alias(String name, long id) {}
        public final List<Alias> aliases;
        private KeySetData(Parcel in) {
            properSigningKeySet = in.readLong(); upgrades = in.createLongArray();
            if (upgrades != null && upgrades.length == 0) throw new IllegalArgumentException("original upgrade array cannot be allocated-empty");
            int count = in.readInt();
            if (count < 0 || count > in.dataAvail() / 12) throw new IllegalArgumentException("invalid keyset alias count");
            var values = new ArrayList<Alias>(count);
            for (int i = 0; i < count; i++) values.add(new Alias(in.readString(), in.readLong()));
            aliases = List.copyOf(values);
        }
        public long[] getUpgradeKeySets() { return upgrades == null ? null : upgrades.clone(); }
    }
    public com.android.server.pm.PackageKeySetData getKeySetData() {
        return com.android.server.pm.CapturedKeySetData.from(keySets);
    }

}
