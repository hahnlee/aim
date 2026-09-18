package android.os;
/** Identity facade for compiling the actual registry in transport-only tests. */
public class Binder implements IBinder {
    public static int getCallingPid() { return 0; }
    public static int getCallingUid() { return 0; }
    public void attachInterface(Object owner, String descriptor) {}
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException { return false; }
    @Override public boolean transact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException { return onTransact(code, data, reply, flags); }
}
