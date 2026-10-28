package android.net;
public final class NetworkCapabilities {
 public int transport=3, owner=1234; public int[] administrators={1234}; public java.util.Set<?> uids=java.util.Set.of(1234);
 public NetworkCapabilities(){} public NetworkCapabilities(NetworkCapabilities nc,long redact){transport=nc.transport; if(redact!=~0L)throw new AssertionError();}
 public boolean satisfiedByNetworkCapabilities(NetworkCapabilities nc){return transport==nc.transport;}
 public NetworkCapabilities setUids(java.util.Set<?> value){uids=value;return this;}
 public NetworkCapabilities setAdministratorUids(int[] value){administrators=value;return this;}
 public NetworkCapabilities setOwnerUid(int value){owner=value;return this;}
}
