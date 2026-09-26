package dev.darwinart.runtime.connectivity;

import android.content.Intent;
import android.net.Proxy;
import android.net.ProxyInfo;
import android.util.Log;
import dev.darwinart.runtime.am.SystemBroadcasts;
import java.util.Objects;

/**
 * ConnectivityService's ProxyTracker for the host network: the effective
 * proxy is published as the sticky PROXY_CHANGE broadcast at start and again
 * whenever macOS reports a proxy configuration change that alters it.
 */
public final class HostProxyService {
    private static final String TAG = "DarwinHostProxy";

    private final ConnectivityState state;
    private final SystemBroadcasts broadcasts;
    private ProxyInfo published;
    private boolean sent;

    public HostProxyService(ConnectivityState state, SystemBroadcasts broadcasts) {
        if (state == null) throw new NullPointerException("state");
        if (broadcasts == null) throw new NullPointerException("broadcasts");
        this.state = state;
        this.broadcasts = broadcasts;
    }

    /** Publishes the current proxy, then follows host changes. */
    public void start() {
        synchronize();
        Thread watcher = new Thread(() -> {
            while (NetworkPathProvider.awaitHostProxyChange()) synchronize();
            Log.w(TAG, "host proxy notifications unavailable");
        }, "DarwinHostProxy");
        watcher.setDaemon(true);
        watcher.start();
    }

    private synchronized void synchronize() {
        ProxyInfo proxy = state.activeNetworkProxy();
        if (sent && Objects.equals(proxy, published)) return;
        published = proxy;
        sent = true;
        // ProxyTracker.sendProxyBroadcast.
        Intent intent = new Intent(Proxy.PROXY_CHANGE_ACTION);
        intent.addFlags(Intent.FLAG_RECEIVER_REPLACE_PENDING
                | Intent.FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT);
        intent.putExtra(Proxy.EXTRA_PROXY_INFO, proxy);
        Log.i(TAG, "proxy " + (proxy == null ? "none" : proxy));
        broadcasts.broadcastAsSystem(intent, true);
    }
}
