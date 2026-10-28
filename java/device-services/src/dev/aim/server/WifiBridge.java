package dev.aim.server;

import android.content.pm.ParceledListSlice;
import android.net.wifi.IWifiStateChangedListener;
import android.os.Handler;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteCallbackList;
import android.os.RemoteException;
import android.util.Log;
import java.util.Collections;

/**
 * ActiveModeWarden's state listener methods at android-16.0.0_r1, using the
 * original RemoteCallbackList for Binder identity, duplicates and death. These
 * cold calls stay in Java; the native wifi owner checks the original caller's
 * permissions and feeds the Mac's radio state in place of ClientModeManager.
 * WifiServiceImpl's empty Passpoint query uses the original ParceledListSlice;
 * no Passpoint providers are configured and their mutations remain unsupported.
 * Copyright (C) The Android Open Source Project, Apache License 2.0.
 */
final class WifiBridge extends IWifiBridge.Stub {
    private static final String TAG = "AimWifi";
    private final Handler mHandler;
    private int mWifiState = -1;
    private final RemoteCallbackList<IWifiStateChangedListener> mWifiStateChangedListeners =
            new RemoteCallbackList<>();

    WifiBridge(Handler handler) { mHandler = handler; }

    @Override public void addWifiStateChangedListener(IBinder binder) {
        Bridge.enforceSystemUid();
        IWifiStateChangedListener listener = IWifiStateChangedListener.Stub.asInterface(binder);
        if (listener == null) throw new IllegalArgumentException("listener must not be null");
        mHandler.post(() -> {
            mWifiStateChangedListeners.register(listener);
            try {
                listener.onWifiStateChanged();
            } catch (RemoteException e) {
                Log.e(TAG, "onWifiStateChanged: remote exception -- " + e);
            }
        });
    }

    @Override public void removeWifiStateChangedListener(IBinder binder) {
        Bridge.enforceSystemUid();
        IWifiStateChangedListener listener = IWifiStateChangedListener.Stub.asInterface(binder);
        if (listener == null) throw new IllegalArgumentException("listener must not be null");
        mHandler.post(() -> mWifiStateChangedListeners.unregister(listener));
    }

    @Override public void onWifiStateChanged(int state) {
        Bridge.enforceSystemUid();
        mHandler.post(() -> {
            if (mWifiState == state) return;
            mWifiState = state;
            final int itemCount = mWifiStateChangedListeners.beginBroadcast();
            for (int i = 0; i < itemCount; i++) {
                try {
                    mWifiStateChangedListeners.getBroadcastItem(i).onWifiStateChanged();
                } catch (RemoteException e) {
                    Log.e(TAG, "onWifiStateChanged: remote exception -- " + e);
                }
            }
            mWifiStateChangedListeners.finishBroadcast();
        });
    }

    @Override public byte[] getPasspointConfigurations() {
        Bridge.enforceSystemUid();
        Parcel out = Parcel.obtain();
        try {
            out.writeTypedObject(new ParceledListSlice<>(Collections.emptyList()), 0);
            return out.marshall();
        } finally {
            out.recycle();
        }
    }
}
