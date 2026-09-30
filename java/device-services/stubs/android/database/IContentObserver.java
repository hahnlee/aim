// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.database;

import android.os.IBinder;

public interface IContentObserver extends android.os.IInterface {
    abstract class Stub extends android.os.Binder implements IContentObserver {
        public static IContentObserver asInterface(IBinder obj) { throw new RuntimeException("stub"); }
    }
}
