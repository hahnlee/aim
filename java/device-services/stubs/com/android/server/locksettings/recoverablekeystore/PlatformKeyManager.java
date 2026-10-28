// Compile-only pinned image API; never packaged with the guest.
package com.android.server.locksettings.recoverablekeystore;
public class PlatformKeyManager {
 private PlatformKeyManager(android.content.Context context, KeyStoreProxy keyStore, com.android.server.locksettings.recoverablekeystore.storage.RecoverableKeyStoreDb db) { throw new RuntimeException("stub"); }
 public static PlatformKeyManager getInstance(android.content.Context context, com.android.server.locksettings.recoverablekeystore.storage.RecoverableKeyStoreDb db) throws java.security.KeyStoreException, java.security.NoSuchAlgorithmException { throw new RuntimeException("stub"); }
 public PlatformEncryptionKey getEncryptKey(int userId) throws java.security.KeyStoreException, java.security.NoSuchAlgorithmException, java.security.UnrecoverableKeyException, java.io.IOException, android.os.RemoteException, InsecureUserException { throw new RuntimeException("stub"); }
}
