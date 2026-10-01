// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.internal.compat;

public interface IPlatformCompat extends android.os.IInterface {
    CompatibilityChangeConfig getAppConfig(android.content.pm.ApplicationInfo appInfo)
            throws android.os.RemoteException;

    abstract class Stub extends android.os.Binder implements IPlatformCompat {
        public static IPlatformCompat asInterface(android.os.IBinder obj) { throw new RuntimeException("stub"); }
    }
}
