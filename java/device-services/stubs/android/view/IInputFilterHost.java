// Compilation ABI, AOSP android-16.0.0_r1 (Apache-2.0).
package android.view;
public interface IInputFilterHost extends android.os.IInterface {
    void sendInputEvent(InputEvent event, int policyFlags) throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IInputFilterHost {
        public android.os.IBinder asBinder() { throw new RuntimeException("stub"); }
    }
}
