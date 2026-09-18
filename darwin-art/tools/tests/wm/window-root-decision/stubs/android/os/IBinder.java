package android.os;

public interface IBinder {
    interface DecisionEndpoint {
        boolean send(dev.darwinart.runtime.wm.DesktopRootFocusDecision decision)
                throws RemoteException;
    }
}
