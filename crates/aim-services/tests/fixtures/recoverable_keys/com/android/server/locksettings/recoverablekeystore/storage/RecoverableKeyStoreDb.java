// Host-only Android/owner boundary double; production bridge compiled unchanged.
package com.android.server.locksettings.recoverablekeystore.storage;
import dev.aim.server.RecoverableKeysTest;
public class RecoverableKeyStoreDb {
 public static final java.util.Set<String> keys=new java.util.HashSet<>();
 public static RecoverableKeyStoreDb newInstance(android.content.Context c){RecoverableKeysTest.event("open");return new RecoverableKeyStoreDb();}
 public boolean removeKey(int uid,String alias){RecoverableKeysTest.event("remove:"+uid+":"+alias);return keys.remove(uid+":"+alias);}
 public long setShouldCreateSnapshot(int user,int uid,boolean flag){if(!flag)throw new AssertionError();RecoverableKeysTest.event("snapshot:"+user+":"+uid);return 1;}
}
