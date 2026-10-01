// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.window;

import android.os.IBinder;

public class WindowOrganizer {
    public IBinder startNewTransition(int type, WindowContainerTransaction t) { throw new RuntimeException("stub"); }
    public void startTransition(IBinder transitionToken, WindowContainerTransaction t) { throw new RuntimeException("stub"); }
    public void finishTransition(IBinder transitionToken, WindowContainerTransaction t) { throw new RuntimeException("stub"); }
    public void registerTransitionPlayer(ITransitionPlayer player) { throw new RuntimeException("stub"); }
}
