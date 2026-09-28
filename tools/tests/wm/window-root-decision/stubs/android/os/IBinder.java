package android.os;

public interface IBinder {
    interface DecisionEndpoint {
        boolean send(dev.aim.runtime.wm.DesktopRootFocusDecision decision)
                throws RemoteException;
    }
}
