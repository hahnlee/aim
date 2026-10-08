// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public class Binder implements IBinder {
    public static IBinder allowBlocking(IBinder binder) { throw new RuntimeException("stub"); }
    public Binder() { throw new RuntimeException("stub"); }
    public static final UserHandle getCallingUserHandle() { throw new RuntimeException("stub"); }
    public static final int getCallingPid() { throw new RuntimeException("stub"); }
    public static final int getCallingUid() { throw new RuntimeException("stub"); }
    public static final long clearCallingIdentity() { throw new RuntimeException("stub"); }
    public static final void restoreCallingIdentity(long token) { throw new RuntimeException("stub"); }
    public void attachInterface(IInterface owner, String descriptor) { throw new RuntimeException("stub"); }
    public IInterface queryLocalInterface(String descriptor) { throw new RuntimeException("stub"); }
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) throws RemoteException { throw new RuntimeException("stub"); }
    public void linkToDeath(DeathRecipient recipient, int flags) { throw new RuntimeException("stub"); }
    public String getInterfaceDescriptor() { throw new RuntimeException("stub"); }
    public boolean unlinkToDeath(DeathRecipient recipient, int flags) { throw new RuntimeException("stub"); }
    public final boolean transact(int code, Parcel data, Parcel reply, int flags) throws RemoteException { throw new RuntimeException("stub"); }
}
