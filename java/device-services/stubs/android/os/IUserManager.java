// Compile-only image API; checked by the device-services build node.
package android.os;
public interface IUserManager extends IInterface {
    abstract class Stub extends Binder implements IUserManager {
        public Stub() { throw new RuntimeException("stub"); }
        public IBinder asBinder() { throw new RuntimeException("stub"); }
    }
}
