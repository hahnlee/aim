package dev.darwinart.runtime.connectivity;

import android.Manifest;
import android.os.Binder;
import android.os.Process;
import dev.darwinart.runtime.am.ApplicationPackages;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/**
 * ConnectivityService's enforceAccessPermission for Binder callers: the
 * attached caller must hold ACCESS_NETWORK_STATE in PermissionManagerService.
 */
public final class CallerConnectivityPermissionEnforcer implements ConnectivityPermissionEnforcer {
    private final ApplicationProcessRegistry processes;

    public CallerConnectivityPermissionEnforcer(ApplicationProcessRegistry applicationProcesses) {
        if (applicationProcesses == null) throw new NullPointerException("applicationProcesses");
        processes = applicationProcesses;
    }

    @Override
    public void enforceAccessNetworkState() {
        int pid = Binder.getCallingPid();
        int uid = Binder.getCallingUid();
        if (pid == Process.myPid()) return;
        processes.requireIdentifiedProcess(pid);
        if (!ApplicationPackages.hasPermission(uid, Manifest.permission.ACCESS_NETWORK_STATE)) {
            throw new SecurityException("ACCESS_NETWORK_STATE not granted to Binder caller");
        }
    }
}
