package android.os;

public class Binder implements IBinder {
    private final java.util.List<DeathRecipient> deaths = new java.util.ArrayList<DeathRecipient>();
    public Runnable duringLink;
    public boolean dead;
    public int lifetimeLinks;
    public int lifetimeUnlinks;
    public Runnable duringUnlink;
    public RuntimeException unlinkFailure;
    @Override public void linkToDeath(DeathRecipient recipient, int flags) throws RemoteException {
        if (dead) throw new RemoteException("dead capability");
        deaths.add(recipient);
        ++lifetimeLinks;
        if (duringLink != null) duringLink.run();
    }
    @Override public boolean unlinkToDeath(DeathRecipient recipient, int flags) {
        ++lifetimeUnlinks;
        if (duringUnlink != null) duringUnlink.run();
        if (unlinkFailure != null) throw unlinkFailure;
        return deaths.remove(recipient);
    }
    public void die() {
        dead = true;
        for (DeathRecipient recipient : new java.util.ArrayList<DeathRecipient>(deaths))
            recipient.binderDied();
    }
    private static int pid;
    private static int uid;
    public static void setCallingIdentity(int valuePid, int valueUid) { pid = valuePid; uid = valueUid; }
    public static int getCallingPid() { return pid; }
    public static int getCallingUid() { return uid; }
    public void attachInterface(Object owner, String descriptor) {}
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException { return false; }
    @Override public boolean transact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException { return onTransact(code, data, reply, flags); }
}
