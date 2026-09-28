package android.os;

/** Test-only Binder base with the unsupported-transaction behavior under test. */
public class Binder implements IBinder {
    public void attachInterface(Object owner, String descriptor) {}

    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        return false;
    }

    @Override
    public boolean transact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        return onTransact(code, data, reply, flags);
    }

    @Override
    public void linkToDeath(DeathRecipient recipient, int flags) throws RemoteException {}

    @Override
    public boolean unlinkToDeath(DeathRecipient recipient, int flags) { return true; }
}
