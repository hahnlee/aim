package dev.aim.server;

import android.app.AppGlobals;
import android.os.Parcel;
import android.os.RemoteException;

/** The SDK sandbox package selected by original PackageManagerService. */
public final class PackageSdkSandbox {
    public static byte[] capture() {
        final String name;
        try {
            name = AppGlobals.getPackageManager().getSdkSandboxPackageName();
        } catch (RemoteException error) {
            throw error.rethrowFromSystemServer();
        }
        Parcel out = Parcel.obtain();
        try {
            out.writeString(name);
            return out.marshall();
        } finally { out.recycle(); }
    }
    private PackageSdkSandbox() {}
}
