// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app.adservices;

import android.os.Binder;

public interface IAdServicesManager {
    abstract class Stub extends Binder implements IAdServicesManager {
        public Stub() { throw new RuntimeException("stub"); }
    }
}
