package dev.aim.server;

/** Original netstats query/counter owners in aim-system; Binder preserves the app caller. */
interface INetworkStatsBridge {
    IBinder openSessionForUsageStats(int flags, String callingPackage);
    void incrementOperationCount(int uid, int tag, int operationCount);
}
