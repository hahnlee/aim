package dev.darwinart.runtime.connectivity;

import android.Manifest;
import android.os.Binder;
import android.os.Process;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import dev.darwinart.runtime.pm.InstalledPackageRecord;
import dev.darwinart.runtime.pm.PackageRecords;

/** Permission owner backed by the installed manifest and attached process identity. */
public final class InstalledConnectivityPermissionEnforcer
        implements ConnectivityPermissionEnforcer {
    private final PackageRecords.Source packages;
    private final ApplicationProcessRegistry processes;

    public InstalledConnectivityPermissionEnforcer(
            PackageRecords.Source installedPackages,
            ApplicationProcessRegistry applicationProcesses) {
        if (installedPackages == null) throw new NullPointerException("installedPackages");
        if (applicationProcesses == null) throw new NullPointerException("applicationProcesses");
        packages = installedPackages;
        processes = applicationProcesses;
    }

    @Override
    public void enforceAccessNetworkState() {
        int pid = Binder.getCallingPid();
        int uid = Binder.getCallingUid();
        if (pid == Process.myPid()) return;

        String packageName = processes.requireIdentifiedProcess(pid);
        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(
                packageName, packages.resolveInstalledPackage(packageName));
        if (installed == null || installed.appId != uid
                || !installed.declaresPermission(Manifest.permission.ACCESS_NETWORK_STATE)) {
            throw new SecurityException("ACCESS_NETWORK_STATE not granted to Binder caller");
        }
    }
}
