package android.net;

import java.io.IOException;
import java.net.Socket;

/** Stable process-local identity for a Darwin host network. */
public final class Network {
    private final int netId;

    public Network(int netId) {
        this.netId = netId;
    }

    public int getNetId() {
        return netId;
    }

    public long getNetworkHandle() {
        return ((long) netId << 32) | 0xcafed00dL;
    }

    public void bindSocket(Socket socket) throws IOException {
        if (socket == null) throw new NullPointerException("socket");
        // Darwin has one host routing table. The socket already follows the
        // active route represented by this process-local Network instance.
    }

    @Override
    public int hashCode() {
        return netId;
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof Network && ((Network) other).netId == netId;
    }
}
