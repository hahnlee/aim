package com.android.server.pm;

import android.os.Binder;
import android.os.CreateAppDataArgs;
import android.os.Environment;
import android.os.IInstalld;
import android.os.Parcel;
import android.os.UserHandle;
import android.os.storage.StorageManager;
import android.os.storage.StorageManagerInternal;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import com.android.server.pm.UserManagerInternal;
import dev.aim.server.IPackageAppDataBridge;

/** Only this invocation's new CE/DE directories acquire a rollback claim. */
@SuppressWarnings("try")
public final class NativePackageAppDataBridge extends IPackageAppDataBridge.Stub {
    private final IInstalld installd;
    private final UserManagerInternal users;
    private final java.util.function.Supplier<StorageManagerInternal> storage;
    private final PackageManagerTracedLock installLock;
    private final java.util.Map<Key,Claim> claims=new java.util.HashMap<>();
    private record Key(String name,int user,long ce) { }
    private record Claim(String volume,String name,int user,int uid,int mask,long ce,long de) { }
    public NativePackageAppDataBridge(IInstalld installd,UserManagerInternal users,
            java.util.function.Supplier<StorageManagerInternal> storage,PackageManagerTracedLock installLock) {
        this.installd=java.util.Objects.requireNonNull(installd);
        this.users=java.util.Objects.requireNonNull(users);
        this.storage=java.util.Objects.requireNonNull(storage);
        this.installLock=java.util.Objects.requireNonNull(installLock);
    }
    private static void owner() {if(Binder.getCallingUid()!=1000)throw new SecurityException("Native app-data owner required");}
    private StorageManagerInternal storage() {
        return java.util.Objects.requireNonNull(storage.get(),"StorageManagerInternal owner unavailable");
    }
    @Override public int appDataFlags(int userId) {
        owner();if(!users.exists(userId))throw new IllegalArgumentException("Unknown user "+userId);
        if(StorageManager.isCeStorageUnlocked(userId)&&storage().isCeStoragePrepared(userId))
            return StorageManager.FLAG_STORAGE_DE|StorageManager.FLAG_STORAGE_CE;
        return users.isUserRunning(userId)?StorageManager.FLAG_STORAGE_DE:0;
    }
    private static java.io.File path(String volume,String name,int user,boolean ce) {
        return new java.io.File(ce?Environment.getDataUserCeDirectory(volume,user):Environment.getDataUserDeDirectory(volume,user),name);
    }
    private static StructStat stat(java.io.File path) throws ErrnoException {
        try {var stat=Os.lstat(path.getAbsolutePath());
            if(!OsConstants.S_ISDIR(stat.st_mode))throw new SecurityException("App data is not an owned directory: "+path);
            return stat;
        } catch(ErrnoException failure) {if(failure.errno==OsConstants.ENOENT)return null;throw failure;}
    }
    private static void input(String name,int user,int app) {
        if(name==null||name.isEmpty()||name.indexOf('/')>=0||name.indexOf('\\')>=0||name.indexOf('\0')>=0||name.equals(".")||name.equals("..")||user<0||app<0)
            throw new IllegalArgumentException("Invalid app-data identity");
    }
    private void cleanupNew(String volume,String name,int user,int app,int requested,StructStat beforeCe,StructStat beforeDe,Throwable original) {
        try {
            int uid=UserHandle.getUid(user,app);int mask=0;long ce=-1;
            if((requested&IInstalld.FLAG_STORAGE_CE)!=0&&beforeCe==null) {
                var current=stat(path(volume,name,user,true));if(current!=null&&current.st_uid==uid) {mask|=IInstalld.FLAG_STORAGE_CE;ce=current.st_ino;}
            }
            if((requested&IInstalld.FLAG_STORAGE_DE)!=0&&beforeDe==null) {
                var current=stat(path(volume,name,user,false));if(current!=null&&current.st_uid==uid)mask|=IInstalld.FLAG_STORAGE_DE;
            }
            if(mask!=0)installd.destroyAppData(volume,name,user,mask,ce);
        } catch(Exception cleanup) {original.addSuppressed(cleanup);}
    }
    @Override public byte[] createAppData(String volume,String name,int user,int flags,int app,String seInfo,int target) throws android.os.RemoteException {
        owner();input(name,user,app);java.util.Objects.requireNonNull(seInfo);
        if(volume!=null&&(volume.indexOf('/')>=0||volume.indexOf('\\')>=0||volume.indexOf('\0')>=0||volume.equals(".")||volume.equals("..")))throw new IllegalArgumentException("Invalid app-data volume identity");
        try(var ignored=installLock.acquireLock()) {
            int requested=flags&(IInstalld.FLAG_STORAGE_CE|IInstalld.FLAG_STORAGE_DE);
            StructStat beforeCe=null,beforeDe=null;
            try {
                if((requested&IInstalld.FLAG_STORAGE_CE)!=0)beforeCe=stat(path(volume,name,user,true));
                if((requested&IInstalld.FLAG_STORAGE_DE)!=0)beforeDe=stat(path(volume,name,user,false));
                var args=new CreateAppDataArgs();args.uuid=volume;args.packageName=name;args.userId=user;args.flags=flags;
                args.appId=app;args.previousAppId=0;args.seInfo=seInfo;args.targetSdkVersion=target;
                final android.os.CreateAppDataResult result;
                try {result=installd.createAppData(args);}
                catch(android.os.RemoteException|RuntimeException failure) {
                    cleanupNew(volume,name,user,app,requested,beforeCe,beforeDe,failure);throw failure;
                }
                if(result==null) {var failure=new IllegalStateException("installd returned no app-data result");cleanupNew(volume,name,user,app,requested,beforeCe,beforeDe,failure);throw failure;}
                if(result.exceptionCode!=0) {var failure=new android.os.ServiceSpecificException(result.exceptionCode,result.exceptionMessage);cleanupNew(volume,name,user,app,requested,beforeCe,beforeDe,failure);throw failure;}
                StructStat afterCe=(requested&IInstalld.FLAG_STORAGE_CE)!=0?stat(path(volume,name,user,true)):null;
                StructStat afterDe=(requested&IInstalld.FLAG_STORAGE_DE)!=0?stat(path(volume,name,user,false)):null;
                if(afterCe!=null&&afterCe.st_ino!=result.ceDataInode)throw new IllegalStateException("installd CE inode differs from directory owner");
                if(afterDe!=null&&afterDe.st_ino!=result.deDataInode)throw new IllegalStateException("installd DE inode differs from directory owner");
                int uid=UserHandle.getUid(user,app);int created=0;
                if(afterCe!=null && beforeCe==null && afterCe.st_uid==uid && afterCe.st_ino==result.ceDataInode)created|=IInstalld.FLAG_STORAGE_CE;
                if(afterDe!=null && beforeDe==null && afterDe.st_uid==uid && afterDe.st_ino==result.deDataInode)created|=IInstalld.FLAG_STORAGE_DE;
                var claim=new Claim(volume,name,user,uid,created,result.ceDataInode,result.deDataInode);
                if(created!=0) {
                    var key=new Key(name,user,result.ceDataInode);
                    if(claims.putIfAbsent(key,claim)!=null)throw new IllegalStateException("App-data rollback claim already owned");
                }
                var parcel=Parcel.obtain();try {parcel.writeLong(result.ceDataInode);parcel.writeLong(result.deDataInode);parcel.writeBoolean(created!=0);return parcel.marshall();}
                finally {parcel.recycle();}
            } catch(ErrnoException failure) {throw new android.os.ParcelableException(new java.io.IOException(failure));}
        }
    }
    @Override public void commitAppData(String name,int user,long ce) {
        owner();try(var ignored=installLock.acquireLock()) {claims.remove(new Key(name,user,ce));}
    }
    @Override public void rollbackAppData(String name,int user,long ce) throws android.os.RemoteException {
        owner();
        try(var ignored=installLock.acquireLock()) {
            var key=new Key(name,user,ce);var claim=claims.get(key);if(claim==null)return;
            try {
                int mask=0;
                if((claim.mask&IInstalld.FLAG_STORAGE_CE)!=0) {
                    var current=stat(path(claim.volume,name,user,true));
                    if(current!=null&&current.st_ino==claim.ce&&current.st_uid==claim.uid)mask|=IInstalld.FLAG_STORAGE_CE;
                }
                if((claim.mask&IInstalld.FLAG_STORAGE_DE)!=0) {
                    var current=stat(path(claim.volume,name,user,false));
                    if(current!=null&&current.st_ino==claim.de&&current.st_uid==claim.uid)mask|=IInstalld.FLAG_STORAGE_DE;
                }
                if(mask!=0)installd.destroyAppData(claim.volume,name,user,mask,claim.ce);
                claims.remove(key);
            } catch(ErrnoException failure) {throw new android.os.ParcelableException(new java.io.IOException(failure));}
        }
    }
}
