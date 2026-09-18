package dev.darwinart.runtime.wm;

import android.os.IBinder;
import android.os.RemoteException;

final class DesktopRootFocusDecisionTransport {
    private DesktopRootFocusDecisionTransport() {}

    static boolean send(IBinder callback, DesktopRootFocusDecision decision)
            throws RemoteException {
        if (!(callback instanceof IBinder.DecisionEndpoint))
            throw new IllegalArgumentException("fixture callback is not a decision endpoint");
        return ((IBinder.DecisionEndpoint) callback).send(decision);
    }
}
