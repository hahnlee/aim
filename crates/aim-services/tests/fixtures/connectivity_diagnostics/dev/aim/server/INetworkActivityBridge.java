package dev.aim.server;
import android.net.*;
public interface INetworkActivityBridge {
 void registerNetworkActivityListener(android.os.IBinder listener);
 void unregisterNetworkActivityListener(android.os.IBinder listener);
 boolean isDefaultNetworkActive(); void onDefaultNetworkChanged(boolean available);
 void registerDiagnostics(android.os.IBinder callback,NetworkCapabilities request,int uid,int pid,int id);
 void unregisterDiagnostics(android.os.IBinder callback,int uid);
 void onDiagnosticsNetworkChanged(Network network,LinkProperties lp,NetworkCapabilities nc,long timestamp);
 void onNetworkConnectivityReported(Network network,int uid,boolean connected,boolean revalidating);
 String formatConnectivityDump(ConnectivityDumpSnapshot snapshot,String[] args);
 abstract class Stub extends android.os.Binder implements INetworkActivityBridge {}
}
