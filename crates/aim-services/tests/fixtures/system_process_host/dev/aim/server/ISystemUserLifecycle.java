package dev.aim.server;
interface ISystemUserLifecycle {
    void onUserUnlocking(int userId) throws android.os.RemoteException;
    void onShutdown();
    abstract class Stub extends android.os.Binder implements ISystemUserLifecycle {}
}
