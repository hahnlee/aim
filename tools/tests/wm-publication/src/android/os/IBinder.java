package android.os;

/** Binder identity and unused wire port for the publication-only controller fixture. */
public interface IBinder {
    int FIRST_CALL_TRANSACTION = 1;
    int FLAG_ONEWAY = 1;
    default boolean transact(int code, Parcel data, Parcel reply, int flags) throws RemoteException {
        return false;
    }
    interface DeathRecipient { void binderDied(); }
    default void linkToDeath(DeathRecipient recipient, int flags) throws RemoteException {}
    default boolean unlinkToDeath(DeathRecipient recipient, int flags) { return true; }
}
