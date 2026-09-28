package android.net;

/** Current connectivity snapshot for legacy CONNECTIVITY_ACTION clients. */
public final class NetworkInfo {
    public enum DetailedState {
        IDLE, SCANNING, CONNECTING, AUTHENTICATING, OBTAINING_IPADDR,
        CONNECTED, SUSPENDED, DISCONNECTING, DISCONNECTED, FAILED, BLOCKED,
        VERIFYING_POOR_LINK, CAPTIVE_PORTAL_CHECK
    }

    private final int type;
    private final boolean connected;

    public NetworkInfo(int type, boolean connected) {
        this.type = type;
        this.connected = connected;
    }

    public boolean isConnected() {
        return connected;
    }

    public int getType() {
        return type;
    }

    public int getSubtype() {
        return 0;
    }

    public DetailedState getDetailedState() {
        return connected ? DetailedState.CONNECTED : DetailedState.DISCONNECTED;
    }
}
