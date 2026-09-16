package android.os;

/** Test-only checked Binder exception. */
public class RemoteException extends Exception {
    public RemoteException() {}

    public RemoteException(String message) {
        super(message);
    }
}
