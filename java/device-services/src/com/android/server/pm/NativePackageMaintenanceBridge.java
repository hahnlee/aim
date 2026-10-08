package com.android.server.pm;

import android.content.pm.dex.IArtManager;
import android.os.Binder;
import android.os.IBinder;
import android.os.UserHandle;
import android.util.ArrayMap;
import com.android.server.art.model.ArtFlags;
import com.android.server.art.model.DexoptResult;
import com.android.server.pm.dex.DexoptOptions;
import dalvik.system.DexFile;
import dalvik.system.VMRuntime;
import dev.aim.server.IPackageMaintenanceBridge;
import dev.aim.server.PackageSnapshots;

/** Independent original ART executes over native package snapshots; no PMS action forwarding. */
@SuppressWarnings("try")
public final class NativePackageMaintenanceBridge extends IPackageMaintenanceBridge.Stub {
    private final PackageSnapshots.Store packages;
    private final IArtManager art;
    private final android.content.Context context;
    private final Installer installer;
    private final PackageManagerTracedLock installLock;
    private final java.io.File parserCache;
    private com.android.server.art.ArtManagerLocal profileOwner;
    public NativePackageMaintenanceBridge(PackageSnapshots.Store packages, IArtManager art,
            android.content.Context context,Installer installer,PackageManagerTracedLock installLock,java.io.File parserCache) {
        this.packages=java.util.Objects.requireNonNull(packages);
        this.art=java.util.Objects.requireNonNull(art);
        this.context=java.util.Objects.requireNonNull(context);
        this.installer=java.util.Objects.requireNonNull(installer);
        this.installLock=java.util.Objects.requireNonNull(installLock);
        this.parserCache=parserCache;
    }
    private static void enforceOwner() {
        if(Binder.getCallingUid()!=1000) throw new SecurityException("Native package maintenance owner required");
    }
    @Override public IBinder getArtManagerBinder() { enforceOwner();return art.asBinder(); }
    @Override public boolean dexoptPackage(String packageName,String compilerFilter,boolean force,
            boolean bootComplete,String splitName,boolean secondaryOnly,int callingUid,int callingPid) {
        enforceOwner();
        int flags=(force?NativePackageConstants.DEXOPT_FORCE:0)
                |(bootComplete?NativePackageConstants.DEXOPT_BOOT_COMPLETE:0);
        if(secondaryOnly) flags|=NativePackageConstants.DEXOPT_ONLY_SECONDARY_DEX
                |NativePackageConstants.DEXOPT_CHECK_FOR_PROFILES_UPDATES|NativePackageConstants.DEXOPT_BOOT_COMPLETE;
        else if(DexFile.isProfileGuidedCompilerFilter(compilerFilter)) flags|=NativePackageConstants.DEXOPT_CHECK_FOR_PROFILES_UPDATES;
        var options=new DexoptOptions(packageName,NativePackageConstants.REASON_CMDLINE,compilerFilter,splitName,flags);
        try(var snapshot=packages.filtered(callingUid,UserHandle.of(UserHandle.getUserId(callingUid)),null)) {
            var state=snapshot.getPackageState(packageName);
            if(state==null || state.getAndroidPackage()==null) return false;
            var params=options.convertToDexoptParams(secondaryOnly?0:ArtFlags.FLAG_SHOULD_INCLUDE_DEPENDENCIES);
            var result=DexOptHelper.getArtManagerLocal().dexoptPackage(snapshot,packageName,params);
            switch(result.getFinalStatus()) {
                case DexoptResult.DEXOPT_FAILED:return false;
                case DexoptResult.DEXOPT_SKIPPED:
                case DexoptResult.DEXOPT_PERFORMED:
                case DexoptResult.DEXOPT_CANCELLED:return true;
                default:throw new IllegalArgumentException("DexoptResult for "+result.getPackageDexoptResults().get(0).getPackageName()+" has unsupported status "+result.getFinalStatus());
            }
        }
    }
    @Override public void notifyDexLoad(String packageName,String[] paths,String[] contexts,
            String loaderIsa,int callingUid,int callingPid) {
        enforceOwner();
        if((paths==null)!=(contexts==null) || (paths!=null && paths.length!=contexts.length))
            throw new IllegalArgumentException("Dex path/context owners differ");
        var manager=DexOptHelper.getDexUseManagerLocal();
        if(manager==null) throw new IllegalStateException("ART dex usage owner unavailable");
        ArrayMap<String,String> map=null;
        if(paths!=null) {map=new ArrayMap<String,String>();for(int i=0;i<paths.length;i++) map.put(paths[i],contexts[i]);}
        try(var snapshot=packages.filtered(callingUid,UserHandle.of(UserHandle.getUserId(callingUid)),null)) {
            var state=snapshot.getPackageState(packageName);
            if(loaderIsa!=null && state!=null) {
                String abi=state.getPrimaryCpuAbi();if(abi==null) abi=android.os.Build.SUPPORTED_ABIS[0];
                String expected=InstructionSets.getDexCodeInstructionSet(VMRuntime.getInstructionSet(abi));
                if(!loaderIsa.equals(expected)) {android.util.Slog.wtf("PackageManager","Invalid loaderIsa in notifyDexLoad call from "+packageName+", uid "+callingUid+": expected "+expected+", got "+loaderIsa);return;}
            }
            manager.notifyDexContainersLoaded(snapshot,packageName,map);
        }
    }
    private synchronized com.android.server.art.ArtManagerLocal profileOwner() {
        packages.getVersion();
        var current = com.android.server.LocalManagerRegistry.getManager(com.android.server.art.ArtManagerLocal.class);
        if (profileOwner != null && current != profileOwner)
            throw new IllegalStateException("ART profile owner replaced or removed");
        if (current != null) profileOwner = current;
        return current;
    }
    @Override public void clearAppProfiles(String packageName) {
        enforceOwner();
        var owner = profileOwner();
        if (owner == null) return;
        try(var snapshot=packages.filtered(1000,UserHandle.of(0),null)) {
            try {
                if (profileOwner() != owner) throw new IllegalStateException("ART profile owner changed");
                owner.clearAppProfiles(snapshot,packageName);
            }
            catch(IllegalArgumentException race) {android.util.Slog.w("PackageManager", "Package disappeared while clearing profiles", race);}
        }
    }
    @Override public boolean checkPermission(String permission,int callingUid,int callingPid) {
        enforceOwner();return context.checkPermission(permission,callingPid,callingUid)==android.content.pm.PackageManager.PERMISSION_GRANTED;
    }
    @Override public void enforcePermission(String permission,int callingUid,int callingPid) {
        enforceOwner();context.enforcePermission(permission,callingPid,callingUid,null);
    }
    @Override public void clearCacheFiles(String packageName,int userId,boolean canAccessInstantApps,int callingUid) {
        enforceOwner();
        try(var snapshot=packages.computer()) {
            var state=snapshot.getPackageStateInternal(packageName);
            if(state!=null && state.getUserStateOrDefault(UserHandle.getUserId(callingUid)).isInstantApp() && !canAccessInstantApps) return;
        }
        try(var ignored=installLock.acquireLock()) {
            for(int cacheFlag:new int[]{NativePackageConstants.FLAG_CLEAR_CACHE_ONLY,NativePackageConstants.FLAG_CLEAR_CODE_CACHE_ONLY}) {
                try(var snapshot=packages.computer()) {
                    var pkg=snapshot.getPackage(packageName);
                    if(pkg==null) continue;
                    var state=snapshot.getPackageStateInternal(pkg.getPackageName());
                    int flags=NativePackageConstants.FLAG_STORAGE_DE|NativePackageConstants.FLAG_STORAGE_CE|NativePackageConstants.FLAG_STORAGE_EXTERNAL|cacheFlag;
                    long inode=state==null?0:state.getUserStateOrDefault(userId).getCeDataInode();
                    try {installer.clearAppData(pkg.getVolumeUuid(),pkg.getPackageName(),userId,flags,inode);}
                    catch(Installer.InstallerException failure) {android.util.Slog.w("PackageManager",String.valueOf(failure));}
                    clearAppProfiles(pkg.getPackageName());
                }
            }
        }
    }
    @Override public byte[] storageSpace(String volumeUuid) {
        enforceOwner();var parcel=android.os.Parcel.obtain();
        try {
            try {long bytes=context.getSystemService(android.os.storage.StorageManager.class).findPathForUuid(volumeUuid).getUsableSpace();
                parcel.writeBoolean(true);parcel.writeLong(bytes);parcel.writeString(null);}
            catch(java.io.IOException failure) {parcel.writeBoolean(false);parcel.writeLong(0);parcel.writeString(failure.toString());}
            return parcel.marshall();
        } finally {parcel.recycle();}
    }
    @Override public boolean freeCacheV2() {enforceOwner();return android.os.SystemProperties.getBoolean("fw.free_cache_v2",true);}
    @Override public boolean preloadsExpired() {enforceOwner();return android.os.SystemProperties.getBoolean("persist.sys.preloads.file_cache_expired",false);}
    @Override public long cachePeriod(String setting,long defaultValue) {enforceOwner();return android.provider.Settings.Global.getLong(context.getContentResolver(),setting,defaultValue);}
    @Override public long wallTimeMillis() {enforceOwner();return System.currentTimeMillis();}
    @Override public void deletePreloadsFileCache() {enforceOwner();android.os.FileUtils.deleteContents(android.os.Environment.getDataPreloadsFileCacheDirectory());}
    @Override public void deleteParserCache() {enforceOwner();android.os.FileUtils.deleteContents(parserCache);}
    @Override public void freeInstalldCache(String volumeUuid,long targetBytes,boolean v2,boolean defyQuota) {
        enforceOwner();try(var ignored=installLock.acquireLock()) {
            try {installer.freeCache(volumeUuid,targetBytes,(v2?NativePackageConstants.FLAG_FREE_CACHE_V2:0)|(defyQuota?NativePackageConstants.FLAG_FREE_CACHE_V2_DEFY_QUOTA:0));}
            catch(Installer.InstallerException ignoredFailure) { }
        }
    }
    @Override public void freeStorageServiceCache(String volumeUuid,long requiredBytes) {
        enforceOwner();com.android.server.LocalServices.getService(android.os.storage.StorageManagerInternal.class).freeCache(volumeUuid,requiredBytes);
    }
    @Override public void noteCacheClearCaller(String packageName,int callingUid,int callingPid) {
        enforceOwner();android.util.EventLog.writeEvent(NativePackageConstants.PM_CLEAR_APP_DATA_CALLER,callingPid,callingUid,packageName);
    }
    @Override public void sendFreeStorageResult(android.content.IntentSender sender,boolean success) {
        enforceOwner();if(sender==null)return;
        var options=android.app.BroadcastOptions.makeBasic();options.setPendingIntentBackgroundActivityLaunchAllowed(false);
        try {sender.sendIntent(null,success?1:0,null,null,options.toBundle(),null,null);}
        catch(android.content.IntentSender.SendIntentException failure) {android.util.Slog.w("PackageManager", "Free storage IntentSender failed", failure);}
    }
}
