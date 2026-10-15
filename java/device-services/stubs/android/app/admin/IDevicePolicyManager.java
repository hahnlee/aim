// Compile-only pinned image API; checked by the device-services node.
package android.app.admin;
import android.content.ComponentName;
import android.os.Binder;
import android.os.IBinder;
import android.os.IInterface;
import android.os.RemoteException;
public interface IDevicePolicyManager extends IInterface {
    ComponentName getDeviceOwnerComponent(boolean callingUserOnly) throws RemoteException;
    boolean packageHasActiveAdmins(String packageName, int userHandle) throws RemoteException;
    abstract class Stub extends Binder implements IDevicePolicyManager {
        public static IDevicePolicyManager asInterface(IBinder binder) { throw new RuntimeException("stub"); }
    }
}
