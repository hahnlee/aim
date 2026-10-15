// Compile-only API of the derived bridge; never included in oracle.dex.
package dev.aim.server;
public final class PackageBootstrapBridge {
    public PackageBootstrapBridge() { throw new RuntimeException("stub"); }
    public PackageBootstrapBridge(com.android.server.pm.verify.domain.DomainVerificationManagerInternal owner) { throw new RuntimeException("stub"); }
    public android.os.IBinder asBinder() { throw new RuntimeException("stub"); }
    public static PackageLocal.SigningOwner signingOwner() { throw new RuntimeException("stub"); }
    public static PackageLocal.SdkDataOwner sdkDataOwner() { throw new RuntimeException("stub"); }
}
