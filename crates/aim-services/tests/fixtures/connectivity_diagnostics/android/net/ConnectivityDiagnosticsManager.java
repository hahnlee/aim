package android.net;
public final class ConnectivityDiagnosticsManager {
 public static final class ConnectivityReport {
  public static final String KEY_NETWORK_VALIDATION_RESULT="networkValidationResult",KEY_NETWORK_PROBES_SUCCEEDED_BITMASK="networkProbesSucceeded",KEY_NETWORK_PROBES_ATTEMPTED_BITMASK="networkProbesAttempted";
  public static final int NETWORK_VALIDATION_RESULT_SKIPPED=3;
  public final Network network; public final long timestamp; public final NetworkCapabilities caps; public final android.os.PersistableBundle extras;
  public ConnectivityReport(Network n,long t,LinkProperties lp,NetworkCapabilities nc,android.os.PersistableBundle bundle){network=n;timestamp=t;caps=nc;extras=bundle;}
 }
 public static final class DataStallReport {}
}
