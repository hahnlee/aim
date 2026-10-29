// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm;
public class PackageManagerShellCommand extends android.os.ShellCommand {
    public PackageManagerShellCommand(android.content.pm.IPackageManager packages,
            android.content.Context context,
            com.android.server.pm.verify.domain.DomainVerificationShell domains) {
        throw new RuntimeException("stub");
    }
    @Override public int onCommand(String command) { throw new RuntimeException("stub"); }
    @Override public void onHelp() { throw new RuntimeException("stub"); }
}
