package dev.aim.server;
import android.os.RemoteException;
interface ISystemProcess {
    int getSyncAdapterStandbyBucket(String name, int user) throws RemoteException;
    void reportSyncScheduled(String name, int user, boolean exempted) throws RemoteException;
    void reportExemptedSyncStart(String name, int user) throws RemoteException;
    void addSyncTempAllowlist(int caller, String name, long duration, int type, int user, boolean sync, int code, String reason) throws RemoteException;
}
