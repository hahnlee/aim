package dev.aim.server;

/** Original file algorithms and guest VFS operations; native owns session state and policy. */
interface IPackageInstallerFiles {
    byte[] verifyChecksums(in byte[] checksumParcel, in byte[] signature);
    void requestChecksums(String guestPath, String installerPackage, int optional, int required,
            in byte[] trustedCertificateParcel, IBinder listener);
    void requestPackageChecksums(in byte[] fileParcel, String installerPackage, int optional, int required,
            in byte[] trustedCertificateParcel, IBinder listener);
    void stageViaHardLink(String source, String stageDirectory);
}
