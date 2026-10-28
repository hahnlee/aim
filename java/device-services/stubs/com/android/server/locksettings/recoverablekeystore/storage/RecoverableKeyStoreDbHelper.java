// Compile-only pinned image API; never packaged with the guest.
package com.android.server.locksettings.recoverablekeystore.storage;
public class RecoverableKeyStoreDbHelper extends android.database.sqlite.SQLiteOpenHelper {
 private RecoverableKeyStoreDbHelper(android.content.Context context) { super(context, null, null, 7); throw new RuntimeException("stub"); }
}
