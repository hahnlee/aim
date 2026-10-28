// Host-only Android/owner boundary double; production bridge compiled unchanged.
package dev.aim.server;
public interface IRecoverableKeyStoreKeys {
 String generateKey(int uid,String alias) throws android.os.RemoteException;
 void removeKey(int uid,String alias) throws android.os.RemoteException;
 abstract class Stub implements IRecoverableKeyStoreKeys {}
}
