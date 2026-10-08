package com.android.server.pm;

import android.content.Context;
import android.os.storage.StorageManager;
import java.util.Objects;

/** Independent original storage, carrier, parser policy, ART and apexd leaves. */
public final class NativePackageBootLifecycleLeaf extends dev.aim.server.IPackageBootLifecycleLeaf.Stub {
    private final Context context;
    private final android.os.Handler handler;
    private android.os.storage.StorageEventListener storageListener;
    private android.content.BroadcastReceiver overlayReceiver;
    public NativePackageBootLifecycleLeaf(Context context, android.os.Handler handler) {
        this.context=Objects.requireNonNull(context); this.handler=Objects.requireNonNull(handler);
    }
    private static void enforce(){if(android.os.Binder.getCallingUid()!=1000)throw new SecurityException("Native boot lifecycle owner required");}
    private StorageManager storage(){return Objects.requireNonNull(context.getSystemService(StorageManager.class));}
    @Override public void releaseCompressedBlocks(String[] paths) {
        enforce();Objects.requireNonNull(paths);
        for(int i=paths.length-1;i>=0;i--)com.android.internal.content.F2fsUtils.releaseCompressedBlocks(context.getContentResolver(),new java.io.File(paths[i]));
    }
    @Override public void prepareReady() {
        enforce();
        com.android.internal.telephony.CarrierAppUtils.disableCarrierAppsUntilPrivileged(context.getOpPackageName(),0,context);
    }
    @Override public synchronized void registerStorageListeners(dev.aim.server.IPackageBootLifecycleEvents events) {
        enforce();Objects.requireNonNull(events);
        if(storageListener!=null)throw new IllegalStateException("storage listener already registered");
        storageListener=new android.os.storage.StorageEventListener(){@Override public void onVolumeStateChanged(android.os.storage.VolumeInfo volume,int oldState,int newState){
            if(volume.getType()!=android.os.storage.VolumeInfo.TYPE_PRIVATE||newState!=android.os.storage.VolumeInfo.STATE_MOUNTED)return;
            try{events.volumeReady(volume.getFsUuid());}catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}
        }};
        storage().registerListener(storageListener);
    }
    @Override public void dexOptimizerReady(){enforce();Objects.requireNonNull(com.android.server.LocalManagerRegistry.getManager(com.android.server.art.ArtManagerLocal.class));}
    @Override public synchronized void registerPackageObservers(dev.aim.server.IPackageBootLifecycleEvents events) {
        enforce();Objects.requireNonNull(events);
        if(overlayReceiver!=null)throw new IllegalStateException("package observer already registered");
        overlayReceiver=new android.content.BroadcastReceiver(){@Override public void onReceive(Context context,android.content.Intent intent){
            String name=intent.getData()==null?null:intent.getData().getSchemeSpecificPart();if(name==null)return;
            try{events.overlayChanged(name,getSendingUserId());}catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}
        }};
        android.content.IntentFilter filter=new android.content.IntentFilter(android.content.Intent.ACTION_OVERLAY_CHANGED);filter.addDataScheme("package");
        context.registerReceiverAsUser(overlayReceiver,android.os.UserHandle.ALL,filter,null,handler);
    }
    @Override public void upgradeDexopt(String reason,dev.aim.server.IPackageDexoptCompletion completion){
        enforce();Objects.requireNonNull(reason);Objects.requireNonNull(completion);
        var art=Objects.requireNonNull(com.android.server.LocalManagerRegistry.getManager(com.android.server.art.ArtManagerLocal.class));
        long started=System.nanoTime();
        try{completion.noteBootDexoptStartTimeNanos(started);}catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}
        art.onBoot(reason,null,null);
    }
    @Override public boolean isHibernationSuppressed(String name){
        enforce();var owner=com.android.server.LocalServices.getService(com.android.server.apphibernation.AppHibernationManagerInternal.class);
        return owner!=null&&owner.isHibernatingGlobally(Objects.requireNonNull(name))&&owner.isOatArtifactDeletionEnabled();
    }
    @Override public void performFstrim(){enforce();try{
        var owner=com.android.internal.content.InstallLocationUtils.getStorageManager();
        if(owner==null){android.util.Slog.e("PackageManager","storageManager service unavailable!");return;}
        long interval=android.provider.Settings.Global.getLong(context.getContentResolver(),android.provider.Settings.Global.FSTRIM_MANDATORY_INTERVAL,3*android.text.format.DateUtils.DAY_IN_MILLIS);
        if(interval>0&&System.currentTimeMillis()-owner.lastMaintenance()>interval)owner.runMaintenance();
    }catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}}
    @Override public void scheduleMaintenance(){enforce();var owner=DexOptHelper.getDexUseManagerLocal();if(owner!=null)owner.systemReady();}
    @Override public int apexSessionState(int id){enforce();var info=ApexManager.getInstance().getStagedSessionInfo(id);
        if(info==null||info.isUnknown)return 0;if(info.isActivationFailed||info.isReverted||info.isRevertFailed)return -1;
        if(info.isActivated||info.isSuccess)return 1;return 2;
    }
    @Override public synchronized void close(){enforce();if(storageListener!=null){storage().unregisterListener(storageListener);storageListener=null;}if(overlayReceiver!=null){context.unregisterReceiver(overlayReceiver);overlayReceiver=null;}}
}
