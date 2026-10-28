// Compile-only pinned image API; never packaged with the guest.
package com.android.server.locksettings.recoverablekeystore.storage;
public class CleanupManager {
 private CleanupManager(RecoverySnapshotStorage snapshots, RecoverableKeyStoreDb db, android.os.UserManager users, ApplicationKeyStorage keys) { throw new RuntimeException("stub"); }
 public static CleanupManager getInstance(android.content.Context context, RecoverySnapshotStorage snapshots, RecoverableKeyStoreDb db, ApplicationKeyStorage keys) { throw new RuntimeException("stub"); }
 public void verifyKnownUsers() { throw new RuntimeException("stub"); }
 public void registerRecoveryAgent(int user, int uid) { throw new RuntimeException("stub"); }
}
