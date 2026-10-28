package android.os;
public class RemoteException extends Exception {
    public RemoteException(String message) { super(message); }
    public RuntimeException rethrowFromSystemServer() { return new IllegalStateException(getMessage(), this); }
}
