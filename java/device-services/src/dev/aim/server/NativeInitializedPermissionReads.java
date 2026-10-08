package dev.aim.server;

import com.android.server.pm.permission.LegacyPermissionSettings;
import java.util.Objects;

/** Retain the original legacy owner passed through readLegacyPermissionsTEMP. */
final class NativeInitializedPermissionReads implements NativePackageManagerInternal.ReadOwner {
    private final LegacyPermissionSettings definitions;
    private final NativePMLegacyRuntimePermissions runtime;
    private final com.android.server.pm.dex.DynamicCodeLogger dynamic;
    private final com.android.server.pm.PackageArchiver archiver;
    private final NativePMMutationProducer mutations;
    NativeInitializedPermissionReads(LegacyPermissionSettings definitions,NativePMLegacyRuntimePermissions runtime,
            com.android.server.pm.dex.DynamicCodeLogger dynamic,com.android.server.pm.PackageArchiver archiver,NativePMMutationProducer mutations){
        this.definitions=Objects.requireNonNull(definitions);this.runtime=Objects.requireNonNull(runtime);this.dynamic=Objects.requireNonNull(dynamic);
        this.archiver=Objects.requireNonNull(archiver);this.mutations=Objects.requireNonNull(mutations);
    }
    @Override public LegacyPermissionSettings getLegacyPermissions(){return definitions;}
    @Override public Object getLegacyPermissionsState(int user){return runtime.state(user);}
    @Override public int getLegacyPermissionsVersion(int user){return runtime.version(user);}
    @Override public com.android.server.pm.dex.DynamicCodeLogger getDynamicCodeLogger(){return dynamic;}
    @Override public android.content.pm.ParceledListSlice<android.content.pm.PackageInstaller.SessionInfo> getHistoricalSessions(int user){return mutations.getHistoricalSessions(user);}
    @Override public com.android.server.pm.PackageArchiver getPackageArchiver(){return archiver;}
}
