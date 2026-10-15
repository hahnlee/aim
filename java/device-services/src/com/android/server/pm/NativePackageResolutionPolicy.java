package com.android.server.pm;
import android.content.Context;
import android.content.PermissionChecker;
import android.os.Binder;
import android.os.UserHandle;
import dev.aim.server.IPackageResolutionPolicy;
/** Independent original UM + PermissionChecker/AppOps preflight, no PMS delegation. */
public final class NativePackageResolutionPolicy extends IPackageResolutionPolicy.Stub {
    private final Context context;private final UserManagerInternal users;
    public NativePackageResolutionPolicy(Context context,UserManagerInternal users) {this.context=java.util.Objects.requireNonNull(context);this.users=java.util.Objects.requireNonNull(users);}
    @Override public boolean isKnownIsolatedComputeApp(int uid) {
        if(Binder.getCallingUid()!=1000)throw new SecurityException("Native UID policy owner required");
        return NativeUserManagerBridge.isKnownIsolatedComputeApp(uid);
    }
    @Override public boolean profilePermission(int uid,int user,String name) {
        if(Binder.getCallingUid()!=1000)throw new SecurityException("Native resolution owner required");
        if(!NativeUserManagerBridge.isSameProfileGroup(UserHandle.getUserId(uid),user))return false;
        return PermissionChecker.checkPermissionForPreflight(context,android.Manifest.permission.INTERACT_ACROSS_PROFILES,
            PermissionChecker.PID_UNKNOWN,uid,name)==PermissionChecker.PERMISSION_GRANTED;
    }
}
