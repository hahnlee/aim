// Compile-only pinned image API; never packaged with the guest.
package com.android.server.locksettings.recoverablekeystore.storage;
public class RecoverableKeyStoreDb {
 private RecoverableKeyStoreDb(RecoverableKeyStoreDbHelper helper) { throw new RuntimeException("stub"); }
 public static RecoverableKeyStoreDb newInstance(android.content.Context context) { throw new RuntimeException("stub"); }
 public boolean removeKey(int uid, String alias) { throw new RuntimeException("stub"); }
 public long setShouldCreateSnapshot(int user, int uid, boolean value) { throw new RuntimeException("stub"); }
}
