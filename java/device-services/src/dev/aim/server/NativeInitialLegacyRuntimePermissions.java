package dev.aim.server;

import com.android.permission.persistence.RuntimePermissionsState;
import java.util.Objects;

/** Restored per-user metadata and SettingBase migration owners before live permissions. */
final class NativeInitialLegacyRuntimePermissions implements NativePackageManagerInternal.LegacyRuntimeOwner {
    private final IPackageBootSession boot;
    NativeInitialLegacyRuntimePermissions(IPackageBootSession boot){this.boot=Objects.requireNonNull(boot);}
    @Override public RuntimePermissionsState getLegacyPermissionsState(int user){
        try{return NativePMLegacyRuntimePermissions.decode(boot.getInitialLegacyRuntimePermissionsStateRecord(user));}
        catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}
    }
    @Override public int getLegacyPermissionsVersion(int user){
        try{return boot.getInitialLegacyPermissionsVersion(user);}
        catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}
    }
}
