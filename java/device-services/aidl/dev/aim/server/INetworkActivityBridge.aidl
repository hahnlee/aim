package dev.aim.server;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.LinkProperties;
import dev.aim.server.ConnectivityDumpSnapshot;

/** LegacyNetworkActivityTracker's Ethernet listener ownership in aim-system (#1509). */
interface INetworkActivityBridge {
    void registerNetworkActivityListener(IBinder listener);
    void unregisterNetworkActivityListener(IBinder listener);
    boolean isDefaultNetworkActive();
    oneway void onDefaultNetworkChanged(boolean available);
    void registerDiagnostics(IBinder callback, in NetworkCapabilities request,
            int uid, int pid, int requestId);
    void unregisterDiagnostics(IBinder callback, int uid);
    oneway void onDiagnosticsNetworkChanged(in Network network, in LinkProperties properties,
            in NetworkCapabilities capabilities, long timestamp);
    oneway void onNetworkConnectivityReported(in Network network, int uid, boolean connected,
            boolean revalidating);
    String formatConnectivityDump(in ConnectivityDumpSnapshot snapshot, in String[] args);
}
