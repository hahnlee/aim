package android.os;

/** Test-only Messenger that records or dispatches callback messages synchronously. */
public class Messenger implements Parcelable {
    public interface Sender {
        void send(Message message) throws RemoteException;
    }

    public static final Creator<Messenger> CREATOR = new Creator<Messenger>() {
        public Messenger createFromParcel(Parcel source) {
            return new Messenger((Sender) source.readObject());
        }
        public Messenger[] newArray(int size) { return new Messenger[size]; }
    };

    private final Sender sender;

    public Messenger() { sender = null; }
    public Messenger(Sender callback) { sender = callback; }

    public void send(Message message) throws RemoteException {
        if (sender != null) sender.send(message);
    }

    public void writeToParcel(Parcel destination, int flags) { destination.writeObject(sender); }
}
