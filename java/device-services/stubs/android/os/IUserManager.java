// Compile-only image API; checked by the device-services build node.
package android.os;
public interface IUserManager extends IInterface {
    android.content.pm.UserInfo createUserWithThrow(String name,String type,int flags) throws RemoteException;
    android.content.pm.UserInfo preCreateUserWithThrow(String type) throws RemoteException;
    android.content.pm.UserInfo createProfileForUserWithThrow(String name,String type,int flags,int user,String[] disallowedPackages) throws RemoteException;
    android.content.pm.UserInfo createRestrictedProfileWithThrow(String name,int parent) throws RemoteException;
    abstract class Stub extends Binder implements IUserManager {
        public Stub() { throw new RuntimeException("stub"); }
        public IBinder asBinder() { throw new RuntimeException("stub"); }
    }
}
