package com.android.server.pm;

import android.content.Context;
import android.os.Binder;
import android.os.Bundle;
import android.os.IRemoteCallback;
import android.os.Parcel;
import android.os.RemoteException;
import android.content.pm.DataLoaderManager;
import android.content.pm.DataLoaderParamsParcel;
import android.content.pm.IDataLoader;
import android.content.pm.IDataLoaderStatusListener;
import android.content.pm.InstallationFileParcel;
import android.content.pm.IPackageInstallerSessionFileSystemConnector;
import android.content.pm.FileSystemControlParcel;
import dev.aim.server.IInstallerPreparationBridge;
import java.util.Objects;
import android.os.incremental.IncrementalFileStorages;

/** Original apexd/StorageManager/DataLoaderManager transport, no PMS owner. */
public final class NativeInstallerPreparationBridge extends IInstallerPreparationBridge.Stub {
    private final Context context;
    private final java.util.Map<Integer,IncrementalFileStorages> incremental = new java.util.HashMap<>();
    public NativeInstallerPreparationBridge(Context context) { this.context = Objects.requireNonNull(context); }
    private static void enforce() { if (Binder.getCallingUid()!=android.os.Process.SYSTEM_UID) throw new SecurityException("native installer preparation requires system UID"); }
    private DataLoaderManager manager() { return Objects.requireNonNull(context.getSystemService(DataLoaderManager.class), "data loader manager unavailable"); }
    private IDataLoader loader(int id) { return Objects.requireNonNull(manager().getDataLoader(id), "data loader unavailable"); }
    @Override public boolean supportsCheckpoint() {
        enforce(); try { return com.android.internal.content.InstallLocationUtils.getStorageManager().supportsCheckpoint(); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public int getRollbackId(int id,boolean enabled,boolean rollback) {
        enforce();
        if(enabled){
            try{return com.android.server.LocalServices.getService(com.android.server.rollback.RollbackManagerInternal.class).notifyStagedSession(id);}
            catch(RuntimeException failure){android.util.Slog.e("NativeInstaller","staged rollback enable failed",failure);return -1;}
        }
        if(rollback){
            var manager=Objects.requireNonNull(context.getSystemService(android.content.rollback.RollbackManager.class));
            for(var info:manager.getRecentlyCommittedRollbacks())if(info.getCommittedSessionId()==id)return info.getRollbackId();
            throw new IllegalStateException("Could not find rollback id for commit session: "+id);
        }
        return -1;
    }
    @Override public void startCheckpoint() {
        enforce(); try { com.android.internal.content.InstallLocationUtils.getStorageManager().startCheckpoint(2); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void submitApex(int root, int[] children, boolean rollback, int rollbackId) {
        enforce(); android.apex.ApexSessionParams params = new android.apex.ApexSessionParams();
        params.sessionId = root; params.childSessionIds = children;
        params.isRollback = rollback; params.rollbackId = rollbackId;
        params.hasRollbackEnabled = !rollback && rollbackId != -1;
        try { ApexManager.getInstance().submitStagedSession(params); }
        catch (PackageManagerException failure) { throw new IllegalStateException("apexd staged verification failed", failure); }
    }
    @Override public void markApexReady(int root) {
        enforce(); try { ApexManager.getInstance().markStagedSessionReady(root); }
        catch (PackageManagerException failure) { throw new IllegalStateException("apexd staged ready failed", failure); }
    }
    @Override public boolean abortApex(int root) { enforce(); return ApexManager.getInstance().abortStagedSession(root); }
    @Override public void markApexSuccessful(int root) { enforce(); ApexManager.getInstance().markStagedSessionSuccessful(root); }
    @Override public boolean prepareStreaming(int id, byte[] encoded, InstallationFileParcel[] added,
            String[] removed, IPackageInstallerSessionFileSystemConnector connector, IRemoteCallback target) {
        enforce(); Objects.requireNonNull(connector); Objects.requireNonNull(target);
        Parcel parcel = Parcel.obtain(); final DataLoaderParamsParcel params;
        try { parcel.unmarshall(encoded, 0, encoded.length); parcel.setDataPosition(0);
            params = parcel.readParcelable(null, DataLoaderParamsParcel.class); parcel.enforceNoDataAvail(); }
        finally { parcel.recycle(); }
        if (params.type != 1) throw new IllegalArgumentException("incremental data loader requires incremental storage ownership");
        IDataLoaderStatusListener callback = new IDataLoaderStatusListener.Stub() {
            @Override public void onStatusChanged(int sessionId, int status) {
                if (sessionId != id) throw new IllegalArgumentException("foreign data loader status");
                try {
                    switch (status) {
                        case 2: FileSystemControlParcel control = new FileSystemControlParcel(); control.callback = connector;
                            loader(id).create(id, params, control, this); break;
                        case 3: loader(id).start(id); break;
                        case 4: loader(id).prepareImage(id, added, removed); break;
                        case 6: case 7: loader(id).destroy(id); break;
                        default: break;
                    }
                    Bundle result = new Bundle(); result.putInt("status", status); target.sendResult(result);
                } catch (RemoteException failure) {
                    android.util.Slog.e("NativeInstaller", "data loader preparation failed", failure);
                    Bundle result = new Bundle(); result.putInt("status", 0); result.putString("message", failure.getMessage());
                    try { target.sendResult(result); } catch (RemoteException callbackFailure) { throw callbackFailure.rethrowFromSystemServer(); }
                }
            }
        };
        return manager().bindToDataLoader(id, params, 0, callback);
    }
    @Override public void destroyStreaming(int id) {
        enforce(); synchronized(this){IncrementalFileStorages storage=incremental.remove(id);if(storage!=null)storage.cleanUpAndMarkComplete();}
        manager().unbindFromDataLoader(id);
    }
    @Override public byte[] getIncrementalInputs(String name,int userId) {
        enforce(); Computer snapshot=NativeUserManagerBridge.snapshotComputer(null);
        var info=name==null?null:snapshot.getPackageInfo(name,0,userId);
        String inherited=info!=null&&info.applicationInfo!=null?new java.io.File(info.applicationInfo.getCodePath()).getParent():null;
        long token=Binder.clearCallingIdentity();String defaults,digesters;
        try{defaults=android.provider.DeviceConfig.getString("package_manager_service","incfs_default_timeouts","");
            digesters=android.provider.DeviceConfig.getString("package_manager_service","known_digesters_list","");}
        finally{Binder.restoreCallingIdentity(token);}
        var records=PerPackageReadTimeouts.parseDigestersList(defaults,digesters);
        var result=new java.util.ArrayList<android.os.incremental.PerUidReadTimeouts>();
        for(var record:records){
            var state=snapshot.getPackageStateInternal(record.packageName);
            if(state==null||state.getAppId()<10000)continue;
            var pkg=state.getPkg();
            if(pkg==null||pkg.getLongVersionCode()<record.versionCodes.minVersionCode||pkg.getLongVersionCode()>record.versionCodes.maxVersionCode)continue;
            if(record.sha256certificate!=null&&!pkg.getSigningDetails().hasSha256Certificate(record.sha256certificate))continue;
            for(int user:UserManagerService.getInstance().getUserIds()){
                if(!state.getUserStateOrDefault(user).isInstalled())continue;
                var timeout=new android.os.incremental.PerUidReadTimeouts();timeout.uid=android.os.UserHandle.getUid(user,state.getAppId());
                timeout.minTimeUs=record.timeouts.minTimeUs;timeout.minPendingTimeUs=record.timeouts.minPendingTimeUs;timeout.maxPendingTimeUs=record.timeouts.maxPendingTimeUs;result.add(timeout);
            }
        }
        Parcel output=Parcel.obtain(),timeouts=Parcel.obtain();
        try{timeouts.writeTypedArray(result.toArray(new android.os.incremental.PerUidReadTimeouts[0]),0);
            output.writeString(inherited);output.writeByteArray(timeouts.marshall());return output.marshall();}
        finally{output.recycle();timeouts.recycle();}
    }
    @Override public synchronized boolean prepareIncremental(int id, String stage, String inherited,
            byte[] encoded, InstallationFileParcel[] added, byte[] encodedTimeouts, IRemoteCallback target) {
        enforce(); Parcel parcel = Parcel.obtain(); final android.content.pm.DataLoaderParams params;
        final android.os.incremental.PerUidReadTimeouts[] timeouts;
        try {
            parcel.unmarshall(encoded,0,encoded.length); parcel.setDataPosition(0);
            params = new android.content.pm.DataLoaderParams(parcel.readParcelable(null,DataLoaderParamsParcel.class));
            parcel.enforceNoDataAvail();
            parcel.unmarshall(encodedTimeouts,0,encodedTimeouts.length); parcel.setDataPosition(0);
            timeouts = parcel.createTypedArray(android.os.incremental.PerUidReadTimeouts.CREATOR); parcel.enforceNoDataAvail();
        } finally { parcel.recycle(); }
        android.os.incremental.StorageHealthCheckParams health = new android.os.incremental.StorageHealthCheckParams();
        health.blockedTimeoutMs=2000;health.unhealthyTimeoutMs=7000;health.unhealthyMonitoringMs=60000;
        IDataLoaderStatusListener listener = new IDataLoaderStatusListener.Stub() {
            @Override public void onStatusChanged(int session,int status) throws RemoteException {
                if(session!=id)throw new IllegalArgumentException("foreign incremental loader");
                Bundle event=new Bundle();event.putInt("status",status);target.sendResult(event);
            }
        };
        android.os.incremental.IStorageHealthListener healthy = new android.os.incremental.IStorageHealthListener.Stub() {
            @Override public void onHealthStatus(int storageId,int status) throws RemoteException {
                boolean system="android".equals(params.getData().packageName);
                if(status==0||system&&(status==1||status==2))return;
                Bundle event=new Bundle();event.putInt("status",9);target.sendResult(event);
            }
        };
        try {
            IncrementalFileStorages storage=incremental.get(id);
            if(storage==null){
                storage=IncrementalFileStorages.initialize(context,new java.io.File(stage),inherited==null?null:new java.io.File(inherited),
                    params,listener,health,healthy,java.util.Arrays.asList(added),Objects.requireNonNull(timeouts),
                    new android.content.pm.IPackageLoadingProgressCallback.Stub(){
                        @Override public void onPackageLoadingProgressChanged(float progress)throws RemoteException{
                            Bundle event=new Bundle();event.putInt("status",4);event.putFloat("progress",progress);target.sendResult(event);
                        }
                    });
                incremental.put(id,storage);
            }else storage.startLoading(params,listener,health,healthy,timeouts);
            return false;
        }catch(java.io.IOException failure){throw new IllegalStateException("incremental storage preparation failed",failure);}
    }
}
