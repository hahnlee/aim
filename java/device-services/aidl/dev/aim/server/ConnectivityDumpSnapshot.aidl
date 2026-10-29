package dev.aim.server;
import android.net.NetworkInfo;
import android.net.LinkProperties;
import android.net.NetworkCapabilities;
import dev.aim.server.ConnectivityDumpRequest;
/** Cold-path copy of the native connectivity owner; originals format network objects. */
parcelable ConnectivityDumpSnapshot {
    int netId = -1;
    @nullable NetworkInfo info;
    @nullable LinkProperties links;
    @nullable NetworkCapabilities capabilities;
    ConnectivityDumpRequest[] requests;
    int[] uids;
    int[] blockedReasons;
    @nullable int[] legacyTypes;
    @nullable String legacyError;
    int providerId;
    String providerName;
    @nullable NetworkCapabilities offerCapabilities;
    long offerScorePolicies;
    int offerKeepConnectedReason;
    int[] offerNeeded;
    long creationTime;
    long createdTime;
    long connectedTime;
    long firstValidationTime;
    long lastValidationTime;
    long scorePolicies;
    int keepConnectedReason;
    int factorySerialNumber = -1;
}
