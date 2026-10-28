// Host-only Android/owner boundary double; production bridge compiled unchanged.
package com.android.server.locksettings.recoverablekeystore.storage;
import dev.aim.server.RecoverableKeysTest;
public class ApplicationKeyStorage {
 public static ApplicationKeyStorage getInstance() throws java.security.KeyStoreException {return new ApplicationKeyStorage();}
 public void setSymmetricKeyEntry(int user,int uid,String alias,byte[] secret) throws java.security.KeyStoreException {
  if(secret.length!=32)throw new AssertionError("key length");
  RecoverableKeysTest.event("store:"+user+":"+uid+":"+alias);
  if(RecoverableKeysTest.failure.equals("store"))throw new java.security.KeyStoreException("store failed");
 }
 public String getGrantAlias(int user,int uid,String alias){RecoverableKeysTest.event("grant:"+user+":"+uid+":"+alias);return "recoverable_key:000000000000CAFE";}
 public void deleteEntry(int user,int uid,String alias){RecoverableKeysTest.event("delete:"+user+":"+uid+":"+alias);if(RecoverableKeysTest.failure.equals("delete"))throw new android.os.ServiceSpecificException(22,"delete failed");}
}
