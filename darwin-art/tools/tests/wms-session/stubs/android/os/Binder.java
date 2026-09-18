package android.os;

public class Binder implements IBinder {
    private final java.util.ArrayList<DeathRecipient> deaths = new java.util.ArrayList<>();
    @Override public synchronized void linkToDeath(DeathRecipient recipient, int flags) {
        deaths.add(recipient);
    }
    @Override public synchronized boolean unlinkToDeath(DeathRecipient recipient, int flags) {
        return deaths.remove(recipient);
    }
    public void die() {
        final java.util.ArrayList<DeathRecipient> snapshot;
        synchronized (this) { snapshot = new java.util.ArrayList<>(deaths); deaths.clear(); }
        for (DeathRecipient recipient : snapshot) recipient.binderDied();
    }
    private static final ThreadLocal<Integer> CALLING_PID = new ThreadLocal<Integer>() {
        @Override protected Integer initialValue() { return 0; }
    };
    private static final ThreadLocal<Integer> CALLING_UID = new ThreadLocal<Integer>() {
        @Override protected Integer initialValue() { return 0; }
    };

    public static void setCallingIdentity(int pid, int uid) {
        CALLING_PID.set(pid);
        CALLING_UID.set(uid);
    }

    public static int getCallingPid() { return CALLING_PID.get(); }
    public static int getCallingUid() { return CALLING_UID.get(); }

    public void attachInterface(Object owner, String descriptor) {}
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException { return false; }

    @Override public boolean transact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException { return onTransact(code, data, reply, flags); }
}
