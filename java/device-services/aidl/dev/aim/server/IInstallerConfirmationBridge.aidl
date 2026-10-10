package dev.aim.server;

/** Original non-PMS leaves used by the native PackageInstallerSession owner. */
interface IInstallerConfirmationBridge {
    boolean isDeviceOwnerOrAffiliated(String installerPackage, int installerUid, int userId);
    boolean isSilentTargetAllowed(String targetPackage, int targetSdkVersion);
    boolean isInstallDisabled(String installerPackage, int installerUid, int userId);
    boolean isDependencyInstallerEnabled();
    boolean isUpdateOwnershipEnabled();
    long getUptimeMillis();
}
