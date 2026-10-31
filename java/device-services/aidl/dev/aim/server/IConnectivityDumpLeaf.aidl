package dev.aim.server;
import android.net.LinkProperties;
/** Cold read-only leaves loaded below the original Connectivity module. */
interface IConnectivityDumpLeaf {
    const int DIAGNOSTICS = 0;
    const int TRAFFIC = 1;
    const int KEEPALIVE = 2;
    const int POLICY = 3;
    const int BPF_STATUS = 4;
    const int ACTIVITY = 5;
    String dump(int section, int netId, in LinkProperties properties, boolean verbose);
}
