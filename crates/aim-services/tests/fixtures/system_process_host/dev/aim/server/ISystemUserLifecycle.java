package dev.aim.server;
interface ISystemUserLifecycle {
    void onUserUnlocking(int userId) throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements ISystemUserLifecycle {}
}
