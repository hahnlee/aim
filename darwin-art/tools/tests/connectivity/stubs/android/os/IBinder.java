package android.os;

/** Test-only Binder constants matching the Android public contract. */
public interface IBinder {
    int FIRST_CALL_TRANSACTION = 1;
    int LAST_CALL_TRANSACTION = 0x00ffffff;
    int INTERFACE_TRANSACTION = 0x5f4e5446;
    int FLAG_ONEWAY = 0x00000001;

    interface DeathRecipient {
        void binderDied();
    }

    boolean transact(int code, Parcel data, Parcel reply, int flags) throws RemoteException;
    void linkToDeath(DeathRecipient recipient, int flags) throws RemoteException;
    boolean unlinkToDeath(DeathRecipient recipient, int flags);
}
