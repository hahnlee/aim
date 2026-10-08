// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm;
public class Installer extends com.android.server.SystemService {
    public void onStart() { throw new RuntimeException("stub"); }
    public Installer(android.content.Context context) { super(context); throw new RuntimeException("stub"); }
    static android.os.CreateAppDataArgs buildCreateAppDataArgs(String uuid, String packageName,
            int userId, int flags, int appId, String seInfo, int targetSdkVersion, boolean usesSdk) {
        throw new RuntimeException("stub");
    }
    public android.os.CreateAppDataResult createAppData(android.os.CreateAppDataArgs args) throws InstallerException {throw new RuntimeException("stub");}
    public void destroyAppData(String uuid,String name,int user,int flags,long ceInode) throws InstallerException {throw new RuntimeException("stub");}
    public void linkNativeLibraryDirectory(String uuid,String name,String path,int user) throws InstallerException {throw new RuntimeException("stub");}
    public static class Batch {
        public Batch() { throw new RuntimeException("stub"); }
        public java.util.concurrent.CompletableFuture<android.os.CreateAppDataResult> createAppData(android.os.CreateAppDataArgs args) {
            throw new RuntimeException("stub");
        }
        public void execute(Installer installer) throws InstallerException { throw new RuntimeException("stub"); }
    }
    public static class InstallerException extends Exception {
        public InstallerException(String detailMessage) { super(detailMessage); }
    }


 public void clearAppData(String volume,String name,int user,int flags,long inode)throws InstallerException{throw new RuntimeException("stub");}
 public void freeCache(String volume,long target,int flags)throws InstallerException{throw new RuntimeException("stub");}

 public boolean migrateLegacyObbData()throws InstallerException{throw new RuntimeException("stub");}
 public void cleanupInvalidPackageDirs(String volume,int user,int flags)throws InstallerException{throw new RuntimeException("stub");}
 public void migrateAppData(String volume,String name,int user,int flags)throws InstallerException{throw new RuntimeException("stub");}
 public void fixupAppData(String volume,int flags)throws InstallerException{throw new RuntimeException("stub");}
}
