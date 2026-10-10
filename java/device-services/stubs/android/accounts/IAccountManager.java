// Compile-only pinned Android AIDL API; original account service runs.
package android.accounts;
public interface IAccountManager extends android.os.IInterface {
    void addSharedAccountsFromParentUser(int parent,int user,String operationPackage) throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IAccountManager {
        public Stub(){throw new RuntimeException("stub");}
        public static IAccountManager asInterface(android.os.IBinder binder){throw new RuntimeException("stub");}
        public android.os.IBinder asBinder(){throw new RuntimeException("stub");}
    }
}
