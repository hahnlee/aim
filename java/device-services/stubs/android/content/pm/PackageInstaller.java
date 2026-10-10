// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

import android.os.Handler;

public class PackageInstaller {
    public static final String EXTRA_WARNINGS = "android.content.pm.extra.WARNINGS";
    public static final class PreapprovalDetails implements android.os.Parcelable {
 private PreapprovalDetails(android.os.Parcel parcel){throw new RuntimeException("stub");}
        public static final android.os.Parcelable.Creator<PreapprovalDetails> CREATOR = null;
        public String getPackageName() { throw new RuntimeException("stub"); }
        public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
        public int describeContents() { throw new RuntimeException("stub"); }
    }
    public static final class InstallConstraints implements android.os.Parcelable {
        public InstallConstraints(boolean idle, boolean foreground, boolean interacting, boolean topVisible, boolean inCall) { throw new RuntimeException("stub"); }
        public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
        public int describeContents() { throw new RuntimeException("stub"); }
    }
    public static final class InstallConstraintsResult implements android.os.Parcelable {
        public InstallConstraintsResult(boolean satisfied) { throw new RuntimeException("stub"); }
        public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
        public int describeContents() { throw new RuntimeException("stub"); }
    }
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

    public static class SessionInfo implements android.os.Parcelable {
 public static final android.os.Parcelable.Creator<SessionInfo> CREATOR=null;
        public SessionInfo() { throw new RuntimeException("stub"); }
        public int installReason;
        public String appPackageName;
        public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
        public int describeContents() { throw new RuntimeException("stub"); }
        public int userId;
        public int installFlags;
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
    }
    public static String ACTION_SESSION_COMMITTED;
    public static String EXTRA_SESSION;
    public static String EXTRA_DATA_LOADER_TYPE;
 public static final int STATUS_PENDING_USER_ACTION=-1;
 public static final String EXTRA_PACKAGE_NAME="android.content.pm.extra.PACKAGE_NAME",EXTRA_STATUS="android.content.pm.extra.STATUS",EXTRA_STATUS_MESSAGE="android.content.pm.extra.STATUS_MESSAGE",EXTRA_LEGACY_STATUS="android.content.pm.extra.LEGACY_STATUS",EXTRA_SESSION_ID="android.content.pm.extra.SESSION_ID",EXTRA_CALLBACK="android.content.pm.extra.CALLBACK",EXTRA_DELETE_FLAGS="android.content.pm.extra.DELETE_FLAGS",EXTRA_UNARCHIVE_ID="android.content.pm.extra.UNARCHIVE_ID",EXTRA_UNARCHIVE_PACKAGE_NAME="android.content.pm.extra.UNARCHIVE_PACKAGE_NAME",EXTRA_UNARCHIVE_ALL_USERS="android.content.pm.extra.UNARCHIVE_ALL_USERS";
 public static class SessionParams implements android.os.Parcelable {
 public static final int PERMISSION_STATE_GRANTED=1;
 public static final int MODE_FULL_INSTALL=1;
 public int installFlags;
 public SessionParams(int mode){throw new RuntimeException("stub");}
 public void setAppPackageName(String name){throw new RuntimeException("stub");}
 public void setAppLabel(CharSequence label){throw new RuntimeException("stub");}
 public int describeContents(){throw new RuntimeException("stub");}
 public void writeToParcel(android.os.Parcel out,int flags){throw new RuntimeException("stub");}}

}
