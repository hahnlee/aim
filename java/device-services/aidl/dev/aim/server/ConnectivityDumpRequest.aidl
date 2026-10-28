package dev.aim.server;
import android.net.NetworkRequest;
parcelable ConnectivityDumpRequest {
    NetworkRequest request;
    int uid;
    int pid;
    int asUid;
    int flags;
    int declared;
    int satisfier = -1;
    boolean tracked;
}
