// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.window;

import android.os.Binder;
import android.os.IBinder;
import android.os.IInterface;
import android.os.RemoteException;
import android.view.SurfaceControl;

public interface ITransitionPlayer extends IInterface {
    void onTransitionReady(IBinder transitionToken, TransitionInfo info, SurfaceControl.Transaction t, SurfaceControl.Transaction finishT) throws RemoteException;
    void requestStartTransition(IBinder transitionToken, TransitionRequestInfo request) throws RemoteException;

    abstract class Stub extends Binder implements ITransitionPlayer {
        public Stub() { throw new RuntimeException("stub"); }
        @Override
        public IBinder asBinder() { throw new RuntimeException("stub"); }
    }
}
