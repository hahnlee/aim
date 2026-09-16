package android.os;

import java.util.ArrayList;

public class Binder implements IBinder {
    private final ArrayList<DeathRecipient> recipients = new ArrayList<>();

    public static long clearCallingIdentity() { return 0; }
    public static void restoreCallingIdentity(long token) {}

    @Override public void linkToDeath(DeathRecipient recipient, int flags)
            throws RemoteException {
        recipients.add(recipient);
    }

    @Override public boolean unlinkToDeath(DeathRecipient recipient, int flags) {
        return recipients.remove(recipient);
    }
}
