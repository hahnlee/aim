package android.view;

/** Test stub: the IWindowId surface WindowIdRegistry implements. */
public interface IWindowId {
    boolean isFocused() throws android.os.RemoteException;
    void registerFocusObserver(IWindowFocusObserver observer) throws android.os.RemoteException;
    void unregisterFocusObserver(IWindowFocusObserver observer) throws android.os.RemoteException;

    abstract class Stub extends android.os.Binder implements IWindowId {}
}
