package com.android.server.pm;
import android.content.Context;
import android.os.Environment;
import android.os.ParcelableException;
import android.os.UserHandle;
import android.os.storage.StorageManager;
import dev.aim.server.PackageSnapshots;
import dev.aim.server.IPackageInternalStorageBridge;
/** Independent storage and ART leaves over native package snapshots. */
public final class NativePackageInternalStorageBridge extends IPackageInternalStorageBridge.Stub {
    private final Context context; private final Installer installer; private final PackageSnapshots.Store packages;
    public NativePackageInternalStorageBridge(Context context, Installer installer, PackageSnapshots.Store packages) {
        this.context=java.util.Objects.requireNonNull(context); this.installer=java.util.Objects.requireNonNull(installer); this.packages=java.util.Objects.requireNonNull(packages);
    }
    private static void enforce() { if(android.os.Binder.getCallingUid()!=1000)throw new SecurityException("Native storage owner required"); }
    @Override public void freeAllAboveQuota(String volume) {
        enforce(); try { installer.freeCache(volume,Long.MAX_VALUE,NativePackageConstants.FLAG_FREE_CACHE_V2|NativePackageConstants.FLAG_FREE_CACHE_DEFY_TARGET_FREE_BYTES); }
        catch(Installer.InstallerException error){throw new ParcelableException(error);}
    }
    @Override public long deleteOat(String name,int callerUid) {
        enforce(); try(var snapshot=packages.filtered(callerUid,UserHandle.of(UserHandle.getUserId(callerUid)),null)) {
            try{return DexOptHelper.getArtManagerLocal().deleteDexoptArtifacts(snapshot,name).getFreedBytes();}
            catch(IllegalArgumentException|IllegalStateException error){android.util.Slog.e("PackageManager",error.toString());return -1;}
        }
    }
    @Override public void migrateObb(){enforce();try{installer.migrateLegacyObbData();}catch(Installer.InstallerException error){throw new ParcelableException(error);}}
    @Override public String[] writableVolumes(){
        enforce(); var volumes=context.getSystemService(StorageManager.class).getWritablePrivateVolumes();String[] result=new String[volumes.size()];
        for(int i=0;i<result.length;i++)result[i]=volumes.get(i).getFsUuid();return result;
    }
    @Override public boolean ceUnlocked(int user){enforce();return StorageManager.isCeStorageUnlocked(user);}
    @Override public boolean fileEncrypted(){enforce();return StorageManager.isFileEncrypted();}
    @Override public void fixupData(String volume,int flags){enforce();try{installer.fixupAppData(volume,flags);}catch(Installer.InstallerException error){throw new ParcelableException(error);}}
    @Override public boolean applyDefaultDeviceStorage(){enforce();return android.content.pm.PackageManager.APPLY_DEFAULT_TO_DEVICE_PROTECTED_STORAGE;}
    @Override public void cleanupInvalid(String volume,int user,int flags){enforce();try{installer.cleanupInvalidPackageDirs(volume,user,flags);}catch(Installer.InstallerException error){throw new ParcelableException(error);}}
    @Override public String[] dataDirectoryNames(String volume,int user,boolean ce){
        enforce();var directory=ce?Environment.getDataUserCeDirectory(volume,user):Environment.getDataUserDeDirectory(volume,user);
        var files=android.os.FileUtils.listFilesOrEmpty(directory);String[] names=new String[files.length];for(int i=0;i<files.length;i++)names[i]=files[i].getName();return names;
    }
    @Override public void destroyData(String volume,String name,int user,int flags,long inode){enforce();try{installer.destroyAppData(volume,name,user,flags,inode);}catch(Installer.InstallerException error){throw new ParcelableException(error);}}
    @Override public long[] prepareData(String volume,String name,int user,int flags,int appId,String seinfo,int targetSdk,boolean usesSdk){
        enforce();var args=Installer.buildCreateAppDataArgs(volume,name,user,flags,appId,seinfo,targetSdk,usesSdk);args.previousAppId=-1;
        try{var result=installer.createAppData(args);return new long[]{result.ceDataInode,result.deDataInode};}
        catch(Installer.InstallerException first){
            // Source recovery destroys mismatched data and retries creation once.
            try{installer.destroyAppData(volume,name,user,flags,0);var result=installer.createAppData(args);return new long[]{result.ceDataInode,result.deDataInode};}
            catch(Installer.InstallerException second){second.addSuppressed(first);throw new ParcelableException(second);}
        }
    }
    @Override public void migrateData(String volume,String name,int user,int target){enforce();try{installer.migrateAppData(volume,name,user,target);}catch(Installer.InstallerException error){throw new ParcelableException(error);}}
    @Override public void prepareContents(String volume,String name,int user,int flags,String abi,String library){
        enforce();if((flags&StorageManager.FLAG_STORAGE_CE)==0||abi==null||dalvik.system.VMRuntime.is64BitAbi(abi))return;
        if(library==null)throw new IllegalStateException("32-bit native library directory unavailable");
        if(!new java.io.File(library).exists())return;
        try{installer.linkNativeLibraryDirectory(volume,name,library,user);}catch(Installer.InstallerException error){throw new ParcelableException(error);}
    }
}
