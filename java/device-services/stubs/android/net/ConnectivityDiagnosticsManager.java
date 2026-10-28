// Compile-only pinned image API; checked against the original image.
package android.net;
public final class ConnectivityDiagnosticsManager {
 public ConnectivityDiagnosticsManager(android.content.Context context, IConnectivityManager service) { throw new RuntimeException("stub"); }
 public static final class ConnectivityReport implements android.os.Parcelable {
  public static final String KEY_NETWORK_VALIDATION_RESULT = "networkValidationResult";
  public static final String KEY_NETWORK_PROBES_SUCCEEDED_BITMASK = "networkProbesSucceeded";
  public static final String KEY_NETWORK_PROBES_ATTEMPTED_BITMASK = "networkProbesAttempted";
  public static final int NETWORK_VALIDATION_RESULT_SKIPPED = 3;
  public ConnectivityReport(Network network, long timestamp, LinkProperties lp, NetworkCapabilities nc, android.os.PersistableBundle extras) { throw new RuntimeException("stub"); }
  public int describeContents() { throw new RuntimeException("stub"); }
  public void writeToParcel(android.os.Parcel p, int flags) { throw new RuntimeException("stub"); }
 }
 public static final class DataStallReport {
  public DataStallReport(Network network, long timestamp, int method, LinkProperties lp, NetworkCapabilities nc, android.os.PersistableBundle extras) { throw new RuntimeException("stub"); }
 }
}
