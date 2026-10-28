// Compile-only pinned image API; never packaged with the guest.
package com.android.server.locksettings.recoverablekeystore;
public class RecoverableKeyGenerator {
 private RecoverableKeyGenerator(javax.crypto.KeyGenerator generator, com.android.server.locksettings.recoverablekeystore.storage.RecoverableKeyStoreDb db) { throw new RuntimeException("stub"); }
 public static RecoverableKeyGenerator newInstance(com.android.server.locksettings.recoverablekeystore.storage.RecoverableKeyStoreDb db) throws java.security.NoSuchAlgorithmException { throw new RuntimeException("stub"); }
 public byte[] generateAndStoreKey(PlatformEncryptionKey key, int user, int uid, String alias, byte[] metadata) throws java.security.KeyStoreException, java.security.InvalidKeyException, RecoverableKeyStorageException { throw new RuntimeException("stub"); }
}
