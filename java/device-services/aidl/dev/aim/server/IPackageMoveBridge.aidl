package dev.aim.server;
import android.content.pm.IPackageMoveObserver;

/** Original storage owner, independent of PackageManagerService. */
interface IPackageMoveBridge {
    void movePrimaryStorage(String volumeUuid, IPackageMoveObserver callback);
}
