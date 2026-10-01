// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public interface IBinder {
    int FIRST_CALL_TRANSACTION = 0x00000001;
    int LAST_CALL_TRANSACTION = 0x00ffffff;
    int INTERFACE_TRANSACTION = ('_' << 24) | ('N' << 16) | ('T' << 8) | 'F';
    int FLAG_ONEWAY = 0x00000001;

    IInterface queryLocalInterface(String descriptor);

    boolean transact(int code, Parcel data, Parcel reply, int flags) throws RemoteException;

    void linkToDeath(DeathRecipient recipient, int flags) throws RemoteException;

    interface DeathRecipient {
        void binderDied();
    }
}
