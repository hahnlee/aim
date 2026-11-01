package dev.aim.server;

import android.content.Context;
import android.os.Binder;
import android.os.RemoteException;
import android.os.UserHandle;
import com.android.server.LocalManagerRegistry;
import com.android.server.art.ArtManagerLocal;
import com.android.server.pm.PackageManagerLocal;
import java.util.Objects;

/** Main-thread construction only. A failed startup must restart the registry's process. */
final class ArtOwnerStartup extends IArtDeletion.Stub {
    private final IDexUsePackages epoch;
    private final long generation;
    private final PackageLocal packages;
    private final ArtManagerLocal art;

    static void start(Context context, IDexUsePackages epoch) throws RemoteException {
        var packages = (PackageLocal) Objects.requireNonNull(
                LocalManagerRegistry.getManager(PackageManagerLocal.class));
        try {
            long generation = epoch.getArtOwnerGeneration();
            var art = NativePackageManagerService.OriginalArtOwner.construct(context, packages);
            epoch.attachArtOwner(generation, new ArtOwnerStartup(epoch, generation, packages, art));
            epoch.checkArtOwner(generation);
        } catch (RemoteException | RuntimeException failure) {
            try { packages.closeEpoch(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            throw failure;
        }
    }

    private ArtOwnerStartup(IDexUsePackages epoch, long generation, PackageLocal packages, ArtManagerLocal art) {
        this.epoch = epoch; this.generation = generation; this.packages = packages; this.art = art;
    }

    private void enforce(long requested) throws RemoteException {
        if (Binder.getCallingUid() != 1000) throw new SecurityException("Native ART owner required");
        if (requested != generation) throw new IllegalStateException("ART process generation differs");
        try { epoch.checkArtOwner(generation); }
        catch (RemoteException | RuntimeException failure) {
            try { packages.closeEpoch(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            finally { android.os.Process.killProcess(android.os.Process.myPid()); }
            throw failure;
        }
    }

    @Override public void clearAppProfiles(long requested, String name) throws RemoteException {
        enforce(requested);
        try (var snapshot = packages.withFilteredSnapshot(1000, UserHandle.of(0))) {
            art.clearAppProfiles(snapshot, name);
        }
    }

    @Override public long deleteDexoptArtifacts(long requested, String name, int uid, int user) throws RemoteException {
        enforce(requested);
        try (var snapshot = packages.withFilteredSnapshot(uid, UserHandle.of(user))) {
            return art.deleteDexoptArtifacts(snapshot, name).getFreedBytes();
        } catch (IllegalArgumentException | IllegalStateException failure) {
            return -1; // NativePackageInternalStorageBridge's original deletion mapping.
        }
    }
}
