// A stub of the image's class for compiling against (docs/build.md, "Java"):
// Compile only; installer_parcel verifies linkage against the original boot classpath.
package android.content.pm;

import android.os.Handler;

public class PackageInstaller {
    public static final boolean ENABLE_REVOCABLE_FD;
    static { ENABLE_REVOCABLE_FD = false; }
    public PackageInstaller(IPackageInstaller installer, String installerPackageName,
            String installerAttributionTag, int userId) { throw new RuntimeException("stub"); }

    public SessionInfo getSessionInfo(int sessionId) { throw new RuntimeException("stub"); }
    public void registerSessionCallback(SessionCallback callback, Handler handler) { throw new RuntimeException("stub"); }

    public abstract static class SessionCallback {
        public SessionCallback() { throw new RuntimeException("stub"); }
        public abstract void onCreated(int sessionId);
        public abstract void onBadgingChanged(int sessionId);
        public abstract void onActiveChanged(int sessionId, boolean active);
        public abstract void onProgressChanged(int sessionId, float progress);
        public abstract void onFinished(int sessionId, boolean success);
    }

public static class SessionParams implements android.os.Parcelable {
public SessionParams(int mode){throw new RuntimeException("stub");}
public static final android.os.Parcelable.Creator<SessionParams> CREATOR=null;
public SessionParams setPermissionState(String name,int state){throw new RuntimeException("stub");}
public int mode;
public int installFlags;
public int installLocation;
public int installReason;
public int installScenario;
public long sizeBytes;
public String appPackageName;
public android.graphics.Bitmap appIcon;
public String appLabel;
public android.net.Uri originatingUri;
public int originatingUid;
public android.net.Uri referrerUri;
public String abiOverride;
public String volumeUuid;
public java.util.List<String> whitelistedRestrictedPermissions;
public int autoRevokePermissionsMode;
public String installerPackageName;
public boolean isMultiPackage;
public boolean isStaged;
public boolean forceQueryableOverride;
public long requiredInstalledVersionCode;
public DataLoaderParams dataLoaderParams;
public int rollbackDataPolicy;
public long rollbackLifetimeMillis;
public int rollbackImpactLevel;
public int requireUserAction;
public int packageSource;
public boolean applicationEnabledSettingPersistent;
public int developmentInstallFlags;
public int unarchiveId;
public String dexoptCompilerFilter;
public boolean isAutoInstallDependenciesEnabled;
public void writeToParcel(android.os.Parcel p,int flags){throw new RuntimeException("stub");}public int describeContents(){throw new RuntimeException("stub");}
}
public static class SessionInfo implements android.os.Parcelable {
public SessionInfo(){throw new RuntimeException("stub");}
public static final android.os.Parcelable.Creator<SessionInfo> CREATOR=null;
public void setSessionErrorCode(int code,String message){throw new RuntimeException("stub");}
public int sessionId;
public int userId;
public String installerPackageName;
public String installerAttributionTag;
public String resolvedBaseCodePath;
public float progress;
public boolean sealed;
public boolean active;
public int mode;
public int installReason;
public int installScenario;
public long sizeBytes;
public String appPackageName;
public android.graphics.Bitmap appIcon;
public CharSequence appLabel;
public int installLocation;
public android.net.Uri originatingUri;
public int originatingUid;
public android.net.Uri referrerUri;
public String[] grantedRuntimePermissions;
public java.util.List<String> whitelistedRestrictedPermissions;
public int autoRevokePermissionsMode;
public int installFlags;
public boolean isMultiPackage;
public boolean isStaged;
public boolean forceQueryable;
public int parentSessionId;
public int[] childSessionIds;
public boolean isSessionApplied;
public boolean isSessionReady;
public boolean isSessionFailed;
public boolean isCommitted;
public boolean isPreapprovalRequested;
public int rollbackDataPolicy;
public long rollbackLifetimeMillis;
public int rollbackImpactLevel;
public long createdMillis;
public int requireUserAction;
public int installerUid;
public int packageSource;
public boolean applicationEnabledSettingPersistent;
public int pendingUserActionReason;
public boolean isAutoInstallingDependenciesEnabled;
        public String getAppPackageName() { throw new RuntimeException("stub"); }
        public String getInstallerPackageName() { throw new RuntimeException("stub"); }
        public int getInstallerUid() { throw new RuntimeException("stub"); }
        public int getInstallReason() { throw new RuntimeException("stub"); }
        public int getMode() { throw new RuntimeException("stub"); }
        public int getPackageSource() { throw new RuntimeException("stub"); }
        public int getOriginatingUid() { throw new RuntimeException("stub"); }
        public boolean isMultiPackage() { throw new RuntimeException("stub"); }
        public boolean isStaged() { throw new RuntimeException("stub"); }
        public boolean isCommitted() { throw new RuntimeException("stub"); }
        public int getParentSessionId() { throw new RuntimeException("stub"); }
        public boolean isApplicationEnabledSettingPersistent() { throw new RuntimeException("stub"); }
        public String getResolvedBaseApkPath() { throw new RuntimeException("stub"); }
public void writeToParcel(android.os.Parcel p,int flags){throw new RuntimeException("stub");}public int describeContents(){throw new RuntimeException("stub");}
}
}
