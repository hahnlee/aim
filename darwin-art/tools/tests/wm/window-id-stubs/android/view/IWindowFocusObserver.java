package android.view;

/** Test stub. */
public interface IWindowFocusObserver {
    void focusGained(android.os.IBinder inputToken) throws android.os.RemoteException;
    void focusLost(android.os.IBinder inputToken) throws android.os.RemoteException;
}
