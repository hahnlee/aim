package dev.aim.server;
import dev.aim.server.IPackageScanSnapshot;
/** Original permission owner, supplied with the native transaction's candidate. */
interface IInstallerPermissionBridge {
    byte[] prepare(IPackageScanSnapshot candidate, IPackageScanSnapshot previous,
        in byte[] installation, boolean crossUserSuspensions);
    void rollback(IPackageScanSnapshot candidate, IPackageScanSnapshot current,
        in String[] packageNames, boolean crossUserSuspensions);
}
