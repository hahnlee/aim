package android.os;
public class Binder implements IBinder {
    public static int getCallingPid() { return 4242; }
    public void attachInterface(Object owner, String descriptor) {}
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        return false;
    }
    public final boolean transact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        return onTransact(code, data, reply, flags);
    }
}
