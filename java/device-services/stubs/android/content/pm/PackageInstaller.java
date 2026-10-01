// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

import android.os.Handler;

public class PackageInstaller {
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

    public static class SessionInfo {
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
}
