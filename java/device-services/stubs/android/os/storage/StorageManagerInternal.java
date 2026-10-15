// Compile-only pinned image API; no implementation shipped.
package android.os.storage;
public abstract class StorageManagerInternal {
 public abstract boolean isCeStoragePrepared(int userId);
 public abstract void prepareAppDataAfterInstall(String packageName,int uid);
 public abstract void freeCache(String volume,long bytes);
 public abstract void prepareUserStorageForMove(String from,String to,java.util.List<android.content.pm.UserInfo> users);
}
