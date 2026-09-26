package dev.darwinart.runtime.connectivity;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkInfo;
import android.net.NetworkRequest;
import android.os.Messenger;

/** Narrow system-server endpoint for the Android 16 IConnectivityManager query slice. */
public final class ConnectivityManagerEndpoint extends Binder {
    static final String DESCRIPTOR = "android.net.IConnectivityManager";
    static final int TRANSACTION_GET_ACTIVE_NETWORK =
            IBinder.FIRST_CALL_TRANSACTION;
    static final int TRANSACTION_GET_ACTIVE_NETWORK_INFO =
            IBinder.FIRST_CALL_TRANSACTION + 2;
    static final int TRANSACTION_GET_ALL_NETWORKS =
            IBinder.FIRST_CALL_TRANSACTION + 8;
    static final int TRANSACTION_GET_LINK_PROPERTIES =
            IBinder.FIRST_CALL_TRANSACTION + 13;
    static final int TRANSACTION_GET_NETWORK_CAPABILITIES =
            IBinder.FIRST_CALL_TRANSACTION + 15;
    static final int TRANSACTION_IS_ACTIVE_NETWORK_METERED =
            IBinder.FIRST_CALL_TRANSACTION + 19;
    static final int TRANSACTION_GET_GLOBAL_PROXY = IBinder.FIRST_CALL_TRANSACTION + 29;
    static final int TRANSACTION_GET_PROXY_FOR_NETWORK = IBinder.FIRST_CALL_TRANSACTION + 31;
    static final int TRANSACTION_REQUEST_NETWORK = IBinder.FIRST_CALL_TRANSACTION + 41;
    static final int TRANSACTION_LISTEN_FOR_NETWORK = IBinder.FIRST_CALL_TRANSACTION + 44;
    static final int TRANSACTION_RELEASE_NETWORK_REQUEST = IBinder.FIRST_CALL_TRANSACTION + 46;

    private final ConnectivityPermissionEnforcer permissions;
    private final ConnectivityState state;
    private final ConnectivityProjection projection;
    private final ConnectivityCallbackRegistry callbacks;

    public ConnectivityManagerEndpoint(ConnectivityPermissionEnforcer permissionEnforcer,
            ConnectivityState connectivityState) {
        this(permissionEnforcer, connectivityState,
                new ConnectivityCallbackRegistry(connectivityState));
    }

    public ConnectivityManagerEndpoint(ConnectivityPermissionEnforcer permissionEnforcer,
            ConnectivityState connectivityState, ConnectivityCallbackRegistry callbackRegistry) {
        if (permissionEnforcer == null) throw new NullPointerException("permissionEnforcer");
        if (connectivityState == null) throw new NullPointerException("connectivityState");
        if (callbackRegistry == null) throw new NullPointerException("callbackRegistry");
        permissions = permissionEnforcer;
        state = connectivityState;
        projection = new ConnectivityProjection();
        callbacks = callbackRegistry;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code >= IBinder.FIRST_CALL_TRANSACTION && code <= IBinder.LAST_CALL_TRANSACTION) {
            data.enforceInterface(DESCRIPTOR);
        }
        if (code == INTERFACE_TRANSACTION) {
            reply.writeString(DESCRIPTOR);
            return true;
        }
        switch (code) {
            case TRANSACTION_GET_ACTIVE_NETWORK: {
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                ConnectivitySnapshot snapshot = state.snapshot();
                callbacks.updateSnapshot(snapshot);
                reply.writeNoException();
                reply.writeTypedObject(callbacks.currentNetwork(), 0);
                return true;
            }
            case TRANSACTION_GET_ACTIVE_NETWORK_INFO: {
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                ConnectivitySnapshot snapshot = state.snapshot();
                callbacks.updateSnapshot(snapshot);
                reply.writeNoException();
                reply.writeTypedObject(projection.activeNetworkInfo(snapshot,
                        callbacks.activeNetworkIncarnation()), 0);
                return true;
            }
            case TRANSACTION_GET_ALL_NETWORKS: {
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                ConnectivitySnapshot snapshot = state.snapshot();
                callbacks.updateSnapshot(snapshot);
                reply.writeNoException();
                reply.writeTypedArray(projection.allNetworks(snapshot,
                        callbacks.activeNetworkIncarnation()), 0);
                return true;
            }
            case TRANSACTION_GET_LINK_PROPERTIES: {
                Network network = data.readTypedObject(Network.CREATOR);
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                ConnectivitySnapshot snapshot = state.snapshot();
                callbacks.updateSnapshot(snapshot);
                reply.writeNoException();
                reply.writeTypedObject(
                        projection.linkProperties(snapshot, network,
                                callbacks.activeNetworkIncarnation()), 0);
                return true;
            }
            case TRANSACTION_GET_NETWORK_CAPABILITIES: {
                Network network = data.readTypedObject(Network.CREATOR);
                data.readString(); // callingPackageName, consumed per Android 16 AIDL.
                data.readString(); // callingAttributionTag, consumed per Android 16 AIDL.
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                ConnectivitySnapshot snapshot = state.snapshot();
                callbacks.updateSnapshot(snapshot);
                reply.writeNoException();
                reply.writeTypedObject(
                        projection.networkCapabilities(snapshot, network,
                                callbacks.activeNetworkIncarnation()), 0);
                return true;
            }
            case TRANSACTION_IS_ACTIVE_NETWORK_METERED: {
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                reply.writeNoException();
                reply.writeBoolean(state.snapshot().isMetered());
                return true;
            }
            case TRANSACTION_REQUEST_NETWORK: {
                int uid = data.readInt();
                NetworkCapabilities capabilities =
                        data.readTypedObject(NetworkCapabilities.CREATOR);
                int requestType = data.readInt();
                Messenger messenger = data.readTypedObject(Messenger.CREATOR);
                int timeoutSec = data.readInt();
                IBinder callerBinder = data.readStrongBinder();
                int legacyType = data.readInt();
                int callbackFlags = data.readInt();
                String callingPackageName = data.readString();
                String callingAttributionTag = data.readString();
                int declaredMethodsFlag = data.readInt();
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                NetworkRequest request = callbacks.requestNetwork(uid, capabilities, requestType,
                        messenger, timeoutSec, callerBinder, legacyType, callbackFlags,
                        callingPackageName, callingAttributionTag, declaredMethodsFlag);
                reply.writeNoException();
                reply.writeTypedObject(request, 1);
                return true;
            }
            case TRANSACTION_LISTEN_FOR_NETWORK: {
                NetworkCapabilities capabilities =
                        data.readTypedObject(NetworkCapabilities.CREATOR);
                Messenger messenger = data.readTypedObject(Messenger.CREATOR);
                IBinder callerBinder = data.readStrongBinder();
                int callbackFlags = data.readInt();
                String callingPackageName = data.readString();
                String callingAttributionTag = data.readString();
                int declaredMethodsFlag = data.readInt();
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                NetworkRequest request = callbacks.listenForNetwork(capabilities, messenger,
                        callerBinder, callbackFlags, callingPackageName, callingAttributionTag,
                        declaredMethodsFlag);
                reply.writeNoException();
                reply.writeTypedObject(request, 1);
                return true;
            }
            case TRANSACTION_GET_GLOBAL_PROXY: {
                // The global proxy is the device-owner override
                // (Settings.Global.GLOBAL_HTTP_PROXY_*); none is set here.
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedObject(null, 1);
                return true;
            }
            case TRANSACTION_GET_PROXY_FOR_NETWORK: {
                // ConnectivityService.getProxyForNetwork: with no global
                // proxy, the network's own proxy; one host network here.
                data.readTypedObject(android.net.Network.CREATOR);
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedObject(state.activeNetworkProxy(), 1);
                return true;
            }
            case TRANSACTION_RELEASE_NETWORK_REQUEST: {
                NetworkRequest request = data.readTypedObject(NetworkRequest.CREATOR);
                data.enforceNoDataAvail();
                permissions.enforceAccessNetworkState();
                callbacks.releaseNetworkRequest(request);
                reply.writeNoException();
                return true;
            }
            default:
                return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
    }
}
