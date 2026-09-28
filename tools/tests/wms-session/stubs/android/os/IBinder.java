package android.os;

public interface IBinder {
    int FIRST_CALL_TRANSACTION = 1;
    int FLAG_ONEWAY = 1;
    interface DeathRecipient { void binderDied(); }
    default void linkToDeath(DeathRecipient recipient, int flags) throws RemoteException {}
    default boolean unlinkToDeath(DeathRecipient recipient, int flags) { return true; }

    boolean transact(int code, Parcel data, Parcel reply, int flags) throws RemoteException;
}
