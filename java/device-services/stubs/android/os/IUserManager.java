// Compile-only image API; checked by the device-services build node.
package android.os;
public interface IUserManager extends IInterface {
    android.content.pm.UserInfo createUserWithThrow(String name,String type,int flags) throws RemoteException;
    android.content.pm.UserInfo preCreateUserWithThrow(String type) throws RemoteException;
    android.content.pm.UserInfo createProfileForUserWithThrow(String name,String type,int flags,int user,String[] disallowedPackages) throws RemoteException;
    android.content.pm.UserInfo createRestrictedProfileWithThrow(String name,int parent) throws RemoteException;
    boolean removeUser(int userId) throws RemoteException;
    int removeUserWhenPossible(int userId, boolean overrideDevicePolicy) throws RemoteException;
    android.content.pm.UserInfo getUserInfo(int user) throws RemoteException;
    android.content.pm.UserProperties getUserPropertiesCopy(int user) throws RemoteException;
    boolean hasUserRestriction(String restriction, int user) throws RemoteException;
    boolean isUserRunning(int user) throws RemoteException;
    boolean isUserUnlockingOrUnlocked(int user) throws RemoteException;
    abstract class Stub extends Binder implements IUserManager {
        public static IUserManager asInterface(IBinder binder) { throw new RuntimeException("stub"); }
        public Stub() { throw new RuntimeException("stub"); }
        public IBinder asBinder() { throw new RuntimeException("stub"); }
    }
}
