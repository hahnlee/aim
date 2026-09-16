package android.net;

public final class Network {
    private final long handle;
    private Network(long value) { handle = value; }
    public static Network fromNetworkHandle(long value) { return new Network(value); }
    public long getNetworkHandle() { return handle; }
}
