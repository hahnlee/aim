package android.os;
import java.util.ArrayList;
public class Binder implements IBinder {
    public static int uid = 1000;
    public boolean dead;
    private final ArrayList<DeathRecipient> deaths = new ArrayList<>();
    public static int getCallingUid() { return uid; }
    public IInterface queryLocalInterface(String descriptor) { throw new AssertionError(); }
    public String getInterfaceDescriptor() { throw new AssertionError(); }
    public boolean transact(int code, Parcel data, Parcel reply, int flags) { throw new AssertionError(); }
    public void dump(java.io.FileDescriptor fd, String[] args) { throw new AssertionError(); }
    public void shellCommand(java.io.FileDescriptor input, java.io.FileDescriptor output,
            java.io.FileDescriptor error, String[] arguments, ShellCallback callback,
            ResultReceiver resultReceiver) { throw new AssertionError(); }
    public void linkToDeath(DeathRecipient recipient, int flags) throws RemoteException {
        if (dead) throw new RemoteException();
        deaths.add(recipient);
    }
    public boolean unlinkToDeath(DeathRecipient recipient, int flags) { return deaths.remove(recipient); }
    public void die() {
        dead = true;
        for (DeathRecipient recipient : new ArrayList<>(deaths)) recipient.binderDied();
        deaths.clear();
    }
}
