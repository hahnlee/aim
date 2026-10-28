/*
 * Copyright (C) 2008 The Android Open Source Project
 * Copyright (C) 2020 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package dev.aim.server;

import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.net.ConnectivityDiagnosticsManager.ConnectivityReport;
import android.net.IConnectivityDiagnosticsCallback;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Handler;
import android.os.IBinder;
import android.os.PersistableBundle;
import android.os.RemoteException;
import android.provider.DeviceConfig;
import android.util.Slog;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.Map;

/**
 * ConnectivityService's diagnostics handler and callback policy, adapted from
 * platform/packages/modules/Connectivity, android-16.0.0_r1, ConnectivityService.java
 * (handleRegister/UnregisterConnectivityDiagnosticsCallback through
 * hasConnectivityDiagnosticsPermissions). The native owner supplies its Ethernet network and
 * shares its request counter. No agent administrators, VPNs, Wi-Fi or data-stall detector exist
 * on this device. Framework matching, redaction, report parceling and Binder death stay original.
 */
final class ConnectivityDiagnostics {
    private final Context mContext;
    private final Handler mHandler;
    private final ISystemProcess mHost;
    private final boolean mAllowSysUi;
    private final Map<IBinder, Registration> mCallbacks = new HashMap<>();
    private Network mNetwork;
    private NetworkCapabilities mCapabilities;
    private ConnectivityReport mReport;

    ConnectivityDiagnostics(Context context, Handler handler, ISystemProcess host) {
        mContext = context;
        mHandler = handler;
        mHost = host;
        mAllowSysUi = allowSysUi(context);
    }

    private final class Registration implements IBinder.DeathRecipient {
        final IConnectivityDiagnosticsCallback callback;
        final NetworkCapabilities request;
        final int uid, pid, id;
        Registration(IBinder binder, NetworkCapabilities caps, int uid, int pid, int id) {
            callback = IConnectivityDiagnosticsCallback.Stub.asInterface(binder);
            request = caps;
            this.uid = uid;
            this.pid = pid;
            this.id = id;
        }
        @Override public void binderDied() { unregister(callback.asBinder(), 1000); }
    }

    void register(IBinder binder, NetworkCapabilities request,
            int uid, int pid, int id) {
        mHandler.post(() -> handleRegister(binder, request, uid, pid, id));
    }

    private synchronized void handleRegister(IBinder binder, NetworkCapabilities request,
            int uid, int pid, int id) {
        if (mCallbacks.containsKey(binder)) {
            release(id);
            return;
        }
        Registration registration = new Registration(binder, request, uid, pid, id);
        mCallbacks.put(binder, registration);
        try {
            binder.linkToDeath(registration, 0);
        } catch (RemoteException e) {
            mCallbacks.remove(binder);
            release(id);
            return;
        }
        sendCached(registration);
    }

    void unregister(IBinder binder, int uid) {
        mHandler.post(() -> handleUnregister(binder, uid));
    }

    private synchronized void handleUnregister(IBinder binder, int uid) {
        Registration registration = mCallbacks.get(binder);
        if (registration == null || uid != 1000 && uid != registration.uid) return;
        mCallbacks.remove(binder);
        binder.unlinkToDeath(registration, 0);
        release(registration.id);
    }

    private void release(int id) {
        try {
            mHost.releaseConnectivityDiagnosticsRequest(id);
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        }
    }

    private boolean has(int pid, int uid, String permission) {
        return mContext.checkPermission(permission, pid, uid) == PackageManager.PERMISSION_GRANTED;
    }

    private boolean matches(Registration r) {
        return mNetwork != null && r.request.satisfiedByNetworkCapabilities(mCapabilities)
                && (has(r.pid, r.uid, "android.permission.NETWORK_STACK")
                    || has(r.pid, r.uid, "android.permission.MAINLINE_NETWORK_STACK")
                    || mAllowSysUi && has(r.pid, r.uid, "android.permission.STATUS_BAR_SERVICE"));
    }

    private synchronized void sendCached(Registration r) {
        if (mCallbacks.get(r.callback.asBinder()) != r || !matches(r) || mReport == null) return;
        try {
            r.callback.onConnectivityReportAvailable(mReport);
        } catch (RemoteException e) {
            Slog.e("AimConnectivityDiagnostics", "Error invoking onConnectivityReportAvailable", e);
        }
    }

    synchronized void networkChanged(Network network, LinkProperties properties,
            NetworkCapabilities capabilities, long timestamp) {
        mNetwork = network;
        mCapabilities = capabilities;
        mReport = null;
        if (network == null) return;
        NetworkCapabilities redacted = new NetworkCapabilities(capabilities, ~0L);
        redacted.setUids(null);
        redacted.setAdministratorUids(new int[0]);
        redacted.setOwnerUid(-1);
        PersistableBundle extras = new PersistableBundle();
        // The Mac uplink is accepted without NetworkMonitor probes. Report SKIPPED, as the
        // original does for a network exempt from validation; never claim HTTP/DNS probes ran.
        extras.putInt(ConnectivityReport.KEY_NETWORK_VALIDATION_RESULT,
                ConnectivityReport.NETWORK_VALIDATION_RESULT_SKIPPED);
        extras.putInt(ConnectivityReport.KEY_NETWORK_PROBES_SUCCEEDED_BITMASK, 0);
        extras.putInt(ConnectivityReport.KEY_NETWORK_PROBES_ATTEMPTED_BITMASK, 0);
        mReport = new ConnectivityReport(network, timestamp, properties, redacted, extras);
        for (Registration r : new ArrayList<>(mCallbacks.values())) sendCached(r);
    }

    synchronized void connectivityReported(Network network, int uid, boolean connected,
            boolean revalidating) {
        if (mNetwork == null || !mNetwork.equals(network)) return;
        // A report agreeing with validation goes only to the reporter's permissioned callbacks.
        for (Registration r : new ArrayList<>(mCallbacks.values())) {
            if ((!revalidating && r.uid != uid) || !matches(r)) continue;
            try {
                r.callback.onNetworkConnectivityReported(network, connected);
            } catch (RemoteException e) {
                Slog.e("AimConnectivityDiagnostics", "Error invoking onNetworkConnectivityReported", e);
            }
            if (!revalidating) sendCached(r);
        }
    }

    /** DeviceConfigUtils.isTetheringFeatureNotChickenedOut, same pinned revision.
     * Its implementation lives in the standalone Connectivity loader; this bridge runs in the
     * system loader. Retain its version/kill-switch policy here, using the original PM queries.
     */
    private static boolean allowSysUi(Context context) {
        int flag;
        try {
            flag = Integer.parseInt(DeviceConfig.getString("tethering",
                    "allow_sysui_connectivity_reports", "0"));
        } catch (NumberFormatException e) {
            flag = 0;
        }
        if (flag == 0 || flag == 1) return true;
        if (flag == -1) return false;
        PackageManager pm = context.getPackageManager();
        for (ResolveInfo info : pm.queryIntentActivities(new Intent(
                "com.android.server.connectivity.intent.action.SERVICE_CONNECTIVITY_RESOURCES_APK"),
                PackageManager.MATCH_SYSTEM_ONLY)) {
            if (!info.activityInfo.applicationInfo.sourceDir.startsWith("/apex/com.android.tethering/")) continue;
            String name = info.activityInfo.applicationInfo.packageName;
            int end = name.indexOf("connectivity");
            if (end < 0) throw new IllegalStateException("Invalid connectivity resources package: " + name);
            String prefix = name.substring(0, end);
            long version = 1000;
            try {
                try {
                    version = pm.getPackageInfo(prefix + "tethering", PackageManager.MATCH_APEX).getLongVersionCode();
                } catch (PackageManager.NameNotFoundException e) {
                    version = pm.getPackageInfo(prefix + "go.tethering", PackageManager.MATCH_APEX).getLongVersionCode();
                }
            } catch (PackageManager.NameNotFoundException e) {
                Slog.e("AimConnectivityDiagnostics", "Failed to resolve tethering module version", e);
            }
            return version >= flag;
        }
        throw new IllegalStateException("No connectivity resource package found");
    }
}
