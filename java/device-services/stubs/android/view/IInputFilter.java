// Compilation ABI, AOSP android-16.0.0_r1 (Apache-2.0).
package android.view;
public interface IInputFilter {
    void install(IInputFilterHost host) throws android.os.RemoteException;
    void uninstall() throws android.os.RemoteException;
    void filterInputEvent(InputEvent event, int policyFlags) throws android.os.RemoteException;
}
