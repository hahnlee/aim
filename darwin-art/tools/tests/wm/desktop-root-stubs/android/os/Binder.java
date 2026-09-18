package android.os;

import java.util.ArrayList;

public class Binder implements IBinder {
    private static final ThreadLocal<Integer> CALLING_PID = new ThreadLocal<Integer>() {
        @Override protected Integer initialValue() { return Integer.valueOf(0); }
    };
    private static final ThreadLocal<Integer> CALLING_UID = new ThreadLocal<Integer>() {
        @Override protected Integer initialValue() { return Integer.valueOf(0); }
    };
    private final ArrayList<DeathRecipient> recipients = new ArrayList<DeathRecipient>();

    public static void setCallingIdentity(int pid, int uid) {
        CALLING_PID.set(Integer.valueOf(pid));
        CALLING_UID.set(Integer.valueOf(uid));
    }
    public static int getCallingPid() { return CALLING_PID.get().intValue(); }
    public static int getCallingUid() { return CALLING_UID.get().intValue(); }

    public void attachInterface(Object owner, String descriptor) {}
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException { return false; }
    @Override public boolean transact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException { return onTransact(code, data, reply, flags); }
    @Override public void linkToDeath(DeathRecipient recipient, int flags)
            throws RemoteException { recipients.add(recipient); }
    @Override public boolean unlinkToDeath(DeathRecipient recipient, int flags) {
        return recipients.remove(recipient);
    }

    public void dieForTest() {
        ArrayList<DeathRecipient> copy = new ArrayList<DeathRecipient>(recipients);
        recipients.clear();
        for (DeathRecipient recipient : copy) recipient.binderDied();
    }
}
