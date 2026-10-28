// Host-only Android/owner boundary double; production bridge compiled unchanged.
package com.android.server.locksettings.recoverablekeystore;
import dev.aim.server.RecoverableKeysTest;
public class PlatformKeyManager {
 public static PlatformKeyManager getInstance(android.content.Context c,com.android.server.locksettings.recoverablekeystore.storage.RecoverableKeyStoreDb db) throws java.security.KeyStoreException,java.security.NoSuchAlgorithmException {return new PlatformKeyManager();}
 public PlatformEncryptionKey getEncryptKey(int user) throws java.security.KeyStoreException,java.security.NoSuchAlgorithmException,java.security.UnrecoverableKeyException,java.io.IOException,android.os.RemoteException,InsecureUserException {
  RecoverableKeysTest.event("platform:"+user);
  switch(RecoverableKeysTest.failure){
   case "insecure":throw new InsecureUserException("Screenlock is not set");
   case "keystore":throw new java.security.KeyStoreException("key store failed");
   case "io":throw new java.io.IOException("database failed");
   case "remote":throw new android.os.RemoteException();
   default:return new PlatformEncryptionKey();
  }
 }
}
