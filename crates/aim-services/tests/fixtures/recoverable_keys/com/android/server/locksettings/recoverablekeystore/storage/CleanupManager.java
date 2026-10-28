// Host-only Android/owner boundary double; production bridge compiled unchanged.
package com.android.server.locksettings.recoverablekeystore.storage;
import dev.aim.server.RecoverableKeysTest;
public class CleanupManager {
 public static CleanupManager getInstance(android.content.Context c,RecoverySnapshotStorage s,RecoverableKeyStoreDb db,ApplicationKeyStorage k){return new CleanupManager();}
 public void verifyKnownUsers(){RecoverableKeysTest.event("verify");}
 public void registerRecoveryAgent(int user,int uid){RecoverableKeysTest.event("register:"+user+":"+uid);}
}
