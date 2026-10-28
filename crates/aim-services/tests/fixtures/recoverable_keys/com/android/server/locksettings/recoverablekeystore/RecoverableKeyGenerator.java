// Host-only Android/owner boundary double; production bridge compiled unchanged.
package com.android.server.locksettings.recoverablekeystore;
import dev.aim.server.RecoverableKeysTest;
import com.android.server.locksettings.recoverablekeystore.storage.RecoverableKeyStoreDb;
public class RecoverableKeyGenerator {
 public static RecoverableKeyGenerator newInstance(RecoverableKeyStoreDb db) throws java.security.NoSuchAlgorithmException {return new RecoverableKeyGenerator();}
 public byte[] generateAndStoreKey(PlatformEncryptionKey key,int user,int uid,String alias,byte[] metadata) throws java.security.KeyStoreException,java.security.InvalidKeyException,RecoverableKeyStorageException {
  if(metadata!=null)throw new AssertionError("generateKey metadata");
  RecoverableKeysTest.event("wrap:"+user+":"+uid+":"+alias);
  if(RecoverableKeysTest.failure.equals("wrap"))throw new java.security.InvalidKeyException("wrap failed");
  RecoverableKeyStoreDb.keys.add(uid+":"+alias);
  return new byte[32];
 }
}
