package dev.aim.server;

// Connectivity module jarjar namespace in the pinned image.
import android.net.connectivity.android.net.INetworkActivityListener;
import android.os.Handler;
import android.os.IBinder;
import android.os.RemoteCallbackList;
import android.os.RemoteException;
import android.util.Slog;
import android.content.Context;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.LinkProperties;

/**
 * ConnectivityService.LegacyNetworkActivityTracker (android-16.0.0_r1),
 * restricted to the app host's Ethernet network. This is a cold control path:
 * keep the original RemoteCallbackList in the system process, including its
 * Binder identity, duplicate registration and death handling, rather than
 * implementing another listener registry in Rust.
 *
 * Ethernet has no idle timer, so it and the absence of a default network are
 * always active. On V+ updateDefaultNetworkActivity reports a new default
 * network even without an idle timer; registration itself reports nothing.
 * There are no permission checks on these three ConnectivityService calls.
 * The bridge is private to the system uid, which forwards them for apps.
 * Copyright (C) The Android Open Source Project, Apache License 2.0.
 */
public final class NetworkActivityBridge extends INetworkActivityBridge.Stub {
    private final Handler mHandler;
    private final ConnectivityDiagnostics mDiagnostics;
    private final RemoteCallbackList<INetworkActivityListener> mNetworkActivityListeners =
            new RemoteCallbackList<>();

    public NetworkActivityBridge(Context context, Handler handler, ISystemProcess host) {
        mHandler = handler;
        mDiagnostics = new ConnectivityDiagnostics(context, handler, host);
    }

    @Override public void registerDiagnostics(IBinder callback, NetworkCapabilities request,
            int uid, int pid, int requestId) {
        Bridge.enforceSystemUid();
        mDiagnostics.register(callback, request, uid, pid, requestId);
    }

    @Override public void unregisterDiagnostics(IBinder callback, int uid) {
        Bridge.enforceSystemUid();
        mDiagnostics.unregister(callback, uid);
    }

    @Override public void onDiagnosticsNetworkChanged(Network network, LinkProperties properties,
            NetworkCapabilities capabilities, long timestamp) {
        Bridge.enforceSystemUid();
        mHandler.post(() -> mDiagnostics.networkChanged(network, properties, capabilities, timestamp));
    }

    @Override public void onNetworkConnectivityReported(Network network, int uid, boolean connected,
            boolean revalidating) {
        Bridge.enforceSystemUid();
        mHandler.post(() -> mDiagnostics.connectivityReported(network, uid, connected, revalidating));
    }

    @Override
    public void registerNetworkActivityListener(IBinder listener) {
        Bridge.enforceSystemUid();
        mNetworkActivityListeners.register(INetworkActivityListener.Stub.asInterface(listener));
    }

    @Override
    public void unregisterNetworkActivityListener(IBinder listener) {
        Bridge.enforceSystemUid();
        mNetworkActivityListeners.unregister(INetworkActivityListener.Stub.asInterface(listener));
    }

    @Override
    public boolean isDefaultNetworkActive() {
        Bridge.enforceSystemUid();
        return true;
    }

    @Override
    public void onDefaultNetworkChanged(boolean available) {
        Bridge.enforceSystemUid();
        if (available) mHandler.post(this::reportNetworkActive);
    }

    /** LegacyNetworkActivityTracker.reportNetworkActive, on the owning handler. */
    private void reportNetworkActive() {
        final int length = mNetworkActivityListeners.beginBroadcast();
        try {
            for (int i = 0; i < length; i++) {
                try {
                    mNetworkActivityListeners.getBroadcastItem(i).onNetworkActive();
                } catch (RemoteException | RuntimeException e) {
                    Slog.e("AimNetworkActivity", "Fail to send network activity to listener " + e);
                }
            }
        } finally {
            mNetworkActivityListeners.finishBroadcast();
        }
    }
}
