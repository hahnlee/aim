package dev.aim.server;

import android.content.Context;
import android.content.pm.PermissionInfo;
import android.os.SystemProperties;
import com.android.server.LocalServices;
import com.android.server.pm.permission.LegacyPermissionManagerInternal;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.Objects;

/** Original PackageManagerShellCommand policy leaves; native code owns package queries. */
public final class PackageShellPolicyBridge extends IPackageShellPolicyBridge.Stub {
    private final Context context;
    public PackageShellPolicyBridge(Context context) { this.context=Objects.requireNonNull(context); }
    @Override public boolean bootCompleted() {
        Bridge.enforceSystemUid(); return SystemProperties.getBoolean("sys.boot_completed",false);
    }
    @Override public void resetRuntimePermissions(int callerPid,int callerUid) {
        Bridge.enforceSystemUid();
        if(callerPid<=0||callerUid<0)throw new IllegalArgumentException("Invalid shell caller");
        context.enforcePermission(android.Manifest.permission.REVOKE_RUNTIME_PERMISSIONS,
                callerPid,callerUid,"revokeRuntimePermission");
        if(callerUid!=0&&callerUid!=1000)context.enforcePermission(
                android.Manifest.permission.INTERACT_ACROSS_USERS_FULL,callerPid,callerUid,
                "resetRuntimePermissions");
        Objects.requireNonNull(LocalServices.getService(LegacyPermissionManagerInternal.class),
                "LegacyPermissionManager owner unavailable").resetRuntimePermissions();
    }
    @Override public String[] runtimePermissions(String[] requested) {
        Bridge.enforceSystemUid();Objects.requireNonNull(requested);
        var permissions=Objects.requireNonNull(LocalServices.getService(PermissionManagerServiceInternal.class),
                "permission classification owner unavailable");
        var dangerous=new HashSet<String>();
        for(var info:permissions.getAllPermissionsWithProtection(PermissionInfo.PROTECTION_DANGEROUS))
            dangerous.add(info.name);
        var result=new ArrayList<String>();
        for(String name:requested)if(dangerous.contains(Objects.requireNonNull(name)))result.add(name);
        return result.toArray(new String[0]);
    }
}
