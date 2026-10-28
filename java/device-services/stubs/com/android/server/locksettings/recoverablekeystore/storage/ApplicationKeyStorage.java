// Compile-only pinned image API; never packaged with the guest.
package com.android.server.locksettings.recoverablekeystore.storage;
public class ApplicationKeyStorage {
 private ApplicationKeyStorage(com.android.server.locksettings.recoverablekeystore.KeyStoreProxy keyStore) { throw new RuntimeException("stub"); }
 public static ApplicationKeyStorage getInstance() throws java.security.KeyStoreException { throw new RuntimeException("stub"); }
 public void setSymmetricKeyEntry(int user, int uid, String alias, byte[] secret) throws java.security.KeyStoreException { throw new RuntimeException("stub"); }
 public String getGrantAlias(int user, int uid, String alias) { throw new RuntimeException("stub"); }
 public void deleteEntry(int user, int uid, String alias) { throw new RuntimeException("stub"); }
}
