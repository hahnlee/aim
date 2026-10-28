package dev.aim.server;
import android.net.wifi.IWifiStateChangedListener;
import android.os.*;
import java.util.Arrays;

/** ActiveModeWarden's initial callback, paired removal, duplicates and failed callback. */
public final class WifiBridgeTest {
    static final class Listener extends IWifiStateChangedListener.Stub {
        int calls;
        boolean fail;
        public void onWifiStateChanged() throws RemoteException {
            calls++;
            if (fail) throw new RemoteException();
        }
    }
    static void check(boolean value) { if (!value) throw new AssertionError(); }
    static void expect(Class<? extends Throwable> kind, Runnable call) {
        try { call.run(); } catch (Throwable e) {
            if (kind.isInstance(e)) return;
            throw new AssertionError(e);
        }
        throw new AssertionError("missing " + kind);
    }
    public static void main(String[] args) {
        Handler handler = new Handler();
        WifiBridge owner = new WifiBridge(handler);
        owner.onWifiStateChanged(1);
        handler.drain();
        Listener a = new Listener();
        Listener b = new Listener();
        owner.addWifiStateChangedListener(a);
        check(a.calls == 0); // WifiThreadRunner posts registration.
        handler.drain();
        check(a.calls == 1);
        owner.addWifiStateChangedListener(a);
        handler.drain();
        check(a.calls == 2); // Every add reports, but the registry has one entry.
        owner.addWifiStateChangedListener(b);
        handler.drain();
        owner.onWifiStateChanged(1);
        handler.drain();
        check(a.calls == 2 && b.calls == 1);
        a.fail = true;
        owner.onWifiStateChanged(3);
        handler.drain();
        check(a.calls == 3 && b.calls == 2); // RemoteException does not stop others.
        owner.removeWifiStateChangedListener(a);
        owner.removeWifiStateChangedListener(a); // Unknown removal is harmless.
        owner.onWifiStateChanged(1);
        handler.drain();
        check(a.calls == 3 && b.calls == 3);
        b.die();
        owner.onWifiStateChanged(3);
        handler.drain();
        check(b.calls == 3);
        // Repeated query stays a nonnull empty original ParceledListSlice.
        byte[] empty = {1, 0, 0, 0, 0, 0, 0, 0};
        check(Arrays.equals(owner.getPasspointConfigurations(), empty));
        check(Arrays.equals(owner.getPasspointConfigurations(), empty));
        expect(IllegalArgumentException.class, () -> owner.addWifiStateChangedListener(null));
        expect(IllegalArgumentException.class, () -> owner.removeWifiStateChangedListener(null));
        Binder.uid = 10100;
        expect(SecurityException.class, () -> owner.addWifiStateChangedListener(a));
        expect(SecurityException.class, () -> owner.removeWifiStateChangedListener(a));
        expect(SecurityException.class, () -> owner.onWifiStateChanged(1));
        expect(SecurityException.class, () -> owner.getPasspointConfigurations());
        handler.drain();
        check(a.calls == 3 && b.calls == 3);
    }
}
