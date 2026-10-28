package android.net;
public interface IConnectivityDiagnosticsCallback extends android.os.IInterface {
 void onConnectivityReportAvailable(ConnectivityDiagnosticsManager.ConnectivityReport report) throws android.os.RemoteException;
 void onDataStallSuspected(ConnectivityDiagnosticsManager.DataStallReport report) throws android.os.RemoteException;
 void onNetworkConnectivityReported(Network network,boolean connected) throws android.os.RemoteException;
 abstract class Stub extends android.os.Binder implements IConnectivityDiagnosticsCallback {
  public android.os.IBinder asBinder(){return this;}
  public static IConnectivityDiagnosticsCallback asInterface(android.os.IBinder binder){return (IConnectivityDiagnosticsCallback)binder;}
 }
}
