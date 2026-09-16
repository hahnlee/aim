package dev.darwinart.runtime.connectivity;

import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkRequest;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Message;
import android.os.Messenger;
import android.os.Parcel;
import android.os.RemoteException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.Map;

/**
 * System-server owner of ConnectivityManager callback registrations.
 *
 * <p>The host contributes only immutable {@link ConnectivitySnapshot} facts. This class owns the
 * Android request id, the lifetime of the registration, binder-death cleanup and the current
 * active-network incarnation.
 */
public final class ConnectivityCallbackRegistry {
    // ConnectivityManager.CallbackHandler callback message ids in Android 16.
    public static final int CALLBACK_AVAILABLE = 2;
    public static final int CALLBACK_LOST = 4;
    public static final int CALLBACK_CAP_CHANGED = 6;
    public static final int CALLBACK_IP_CHANGED = 7;
    public static final int CALLBACK_BLK_CHANGED = 11;

    private static final String REQUEST_KEY = "NetworkRequest";
    private static final String NETWORK_KEY = "Network";
    private static final String CAPABILITIES_KEY = "NetworkCapabilities";
    private static final String LINK_PROPERTIES_KEY = "LinkProperties";

    private final ConnectivityState state;
    private final ConnectivityState.Listener stateListener;
    private final ConnectivityProjection projection = new ConnectivityProjection();
    private final Map<NetworkRequest, Registration> registrations =
            new HashMap<NetworkRequest, Registration>();
    private int nextRequestId = 1;
    private int activeNetworkId = ConnectivityProjection.ANDROID_NETWORK_ID;
    private boolean activeNetworkSeen;
    private ConnectivitySnapshot snapshot = ConnectivitySnapshot.unavailable();

    public ConnectivityCallbackRegistry(ConnectivityState connectivityState) {
        if (connectivityState == null) throw new NullPointerException("connectivityState");
        state = connectivityState;
        stateListener = new ConnectivityState.Listener() {
            @Override
            public void onConnectivityChanged(ConnectivitySnapshot changedSnapshot) {
                updateSnapshot(changedSnapshot);
            }
        };
        state.addListener(stateListener);
    }

    /** Refreshes facts from the host provider and emits only lifecycle changes. */
    public synchronized void refresh() {
        updateSnapshot(state.snapshot());
    }

    /** Publishes a host snapshot; useful for the provider's serialized update callback. */
    public synchronized void updateSnapshot(ConnectivitySnapshot value) {
        if (value == null) throw new NullPointerException("snapshot");
        boolean oldActive = snapshot.hasActiveNetwork();
        boolean newActive = value.hasActiveNetwork();
        if (oldActive == newActive && sameFacts(snapshot, value)) return;
        snapshot = value;
        if (oldActive && !newActive) {
            for (Registration registration : new ArrayList<Registration>(registrations.values())) {
                sendLost(registration);
            }
        } else if (!oldActive && newActive) {
            // Network handles are incarnations, not a permanent host identity.
            if (activeNetworkSeen) {
                if (activeNetworkId == Integer.MAX_VALUE) activeNetworkId = 1;
                else activeNetworkId++;
            } else {
                activeNetworkSeen = true;
            }
            for (Registration registration :
                    new ArrayList<Registration>(registrations.values())) {
                sendInitial(registration);
            }
        } else if (newActive) {
            for (Registration registration :
                    new ArrayList<Registration>(registrations.values())) {
                if (isSatisfied(registration)) {
                    if (registration.delivered) sendChanged(registration);
                    else sendInitial(registration);
                } else {
                    sendLost(registration);
                }
            }
        }
    }

    public synchronized NetworkRequest requestNetwork(int uid, NetworkCapabilities capabilities,
            int requestType, Messenger messenger, int timeoutSec, IBinder callerBinder,
            int legacyType, int callbackFlags, String callingPackageName,
            String callingAttributionTag, int declaredMethodsFlag) throws RemoteException {
        return register(capabilities, messenger, callerBinder, requestType, legacyType, false);
    }

    public synchronized NetworkRequest listenForNetwork(NetworkCapabilities capabilities,
            Messenger messenger, IBinder callerBinder, int callbackFlags,
            String callingPackageName, String callingAttributionTag, int declaredMethodsFlag)
            throws RemoteException {
        return register(capabilities, messenger, callerBinder, 1, -1, true);
    }

    public synchronized void releaseNetworkRequest(NetworkRequest request) {
        if (request == null) return;
        Registration registration = registrations.remove(request);
        if (registration != null && registration.callerBinder != null && !registration.dead) {
            registration.callerBinder.unlinkToDeath(registration.deathRecipient, 0);
        }
    }

    public synchronized int registrationCount() {
        return registrations.size();
    }

    public synchronized int activeNetworkIncarnation() {
        return activeNetworkId;
    }

    /** Returns the currently published Android network handle, after refreshing host facts. */
    public synchronized Network activeNetwork() {
        refresh();
        return snapshot.hasActiveNetwork() ? incarnationNetwork() : null;
    }

    /** Returns the last published handle without polling the provider again. */
    public synchronized Network currentNetwork() {
        return snapshot.hasActiveNetwork() ? incarnationNetwork() : null;
    }

    private NetworkRequest register(NetworkCapabilities capabilities, Messenger messenger,
            IBinder callerBinder, int requestType, int legacyType, boolean listen)
            throws RemoteException {
        boolean tracksDefault = requestType == 2 || requestType == 5;
        if (capabilities == null) {
            if (!tracksDefault) throw new IllegalArgumentException("networkCapabilities");
            // ConnectivityManager deliberately sends no capabilities for TRACK_DEFAULT and
            // TRACK_SYSTEM_DEFAULT. ConnectivityService supplies its default request here; an
            // unconstrained framework object is sufficient because default tracking follows the
            // selected network rather than capability matching.
            capabilities = new NetworkCapabilities();
        }
        if (messenger == null) throw new IllegalArgumentException("messenger");
        refresh();
        String typeName = listen ? "LISTEN" : typeNameForOrdinal(requestType);
        NetworkRequest request = copyWithRequestId(capabilities, legacyType, nextRequestId++, typeName);
        final Registration registration =
                new Registration(request, messenger, callerBinder, tracksDefault);
        registration.deathRecipient = new IBinder.DeathRecipient() {
            @Override
            public void binderDied() {
                synchronized (ConnectivityCallbackRegistry.this) {
                    registration.dead = true;
                    registrations.remove(registration.request);
                }
            }
        };
        if (callerBinder != null) callerBinder.linkToDeath(registration.deathRecipient, 0);
        registrations.put(request, registration);
        sendInitial(registration);
        return request;
    }

    private boolean isSatisfied(Registration registration) {
        if (!snapshot.hasActiveNetwork()) return false;
        if (registration.tracksDefault) return true;
        NetworkCapabilities capabilities = projection.networkCapabilities(snapshot, incarnationNetwork());
        return capabilities != null && registration.request.canBeSatisfiedBy(capabilities);
    }

    private Network incarnationNetwork() {
        return projection.networkForIncarnation(activeNetworkId);
    }

    private void sendInitial(Registration registration) {
        if (registration.delivered || !isSatisfied(registration)) return;
        Network network = incarnationNetwork();
        NetworkCapabilities capabilities = projection.networkCapabilities(snapshot, network);
        LinkProperties linkProperties = projection.linkProperties(snapshot, network);
        try {
            sendAvailable(registration, network, capabilities, linkProperties);
            registration.delivered = true;
        } catch (RemoteException error) {
            removeAfterDeliveryFailure(registration);
        }
    }

    private void sendAvailable(Registration registration, Network network,
            NetworkCapabilities capabilities, LinkProperties linkProperties)
            throws RemoteException {
        Message message = Message.obtain();
        message.what = CALLBACK_AVAILABLE;
        // Android 16's NetworkCallback dispatch treats zero as the unblocked state.
        message.arg1 = 0;
        Bundle data = message.getData();
        data.putParcelable(REQUEST_KEY, registration.request);
        data.putParcelable(NETWORK_KEY, network);
        data.putParcelable(CAPABILITIES_KEY, capabilities);
        data.putParcelable(LINK_PROPERTIES_KEY, linkProperties);
        registration.messenger.send(message);
    }

    private void sendChanged(Registration registration) {
        Network network = incarnationNetwork();
        try {
            send(registration, CALLBACK_CAP_CHANGED,
                    network, projection.networkCapabilities(snapshot, network), 0);
            send(registration, CALLBACK_IP_CHANGED,
                    network, projection.linkProperties(snapshot, network), 0);
        } catch (RemoteException error) {
            removeAfterDeliveryFailure(registration);
        }
    }

    private void sendLost(Registration registration) {
        if (!registration.delivered) return;
        try {
            send(registration, CALLBACK_LOST, incarnationNetwork(), null, 0);
            registration.delivered = false;
        } catch (RemoteException error) {
            removeAfterDeliveryFailure(registration);
        }
    }

    private void send(Registration registration, int what, Network network, Object payload,
            int arg1) throws RemoteException {
        Message message = Message.obtain();
        message.what = what;
        message.arg1 = arg1;
        Bundle data = message.getData();
        data.putParcelable(REQUEST_KEY, registration.request);
        if (network != null) data.putParcelable(NETWORK_KEY, network);
        if (payload instanceof NetworkCapabilities) {
            data.putParcelable(CAPABILITIES_KEY, (NetworkCapabilities) payload);
        } else if (payload instanceof LinkProperties) {
            data.putParcelable(LINK_PROPERTIES_KEY, (LinkProperties) payload);
        }
        registration.messenger.send(message);
    }

    private void removeAfterDeliveryFailure(Registration registration) {
        if (registrations.remove(registration.request) != null
                && registration.callerBinder != null && !registration.dead) {
            registration.callerBinder.unlinkToDeath(registration.deathRecipient, 0);
        }
    }

    private static boolean sameFacts(ConnectivitySnapshot first, ConnectivitySnapshot second) {
        return first.isMetered() == second.isMetered()
                && first.isConstrained() == second.isConstrained()
                && first.isValidated() == second.isValidated()
                && first.interfaceMask() == second.interfaceMask()
                && equal(first.interfaceName(), second.interfaceName())
                && Arrays.equals(first.dnsServers(), second.dnsServers());
    }

    private static boolean equal(Object first, Object second) {
        return first == null ? second == null : first.equals(second);
    }

    private static String typeNameForOrdinal(int ordinal) {
        switch (ordinal) {
            case 1: return "LISTEN";
            case 2: return "TRACK_DEFAULT";
            case 3: return "REQUEST";
            case 4: return "BACKGROUND_REQUEST";
            case 5: return "TRACK_SYSTEM_DEFAULT";
            case 6: return "LISTEN_FOR_BEST";
            case 7: return "RESERVATION";
            default: return "REQUEST";
        }
    }

    /** Rebuilds the framework parcelable without reflection or a second request-id authority. */
    private static NetworkRequest copyWithRequestId(NetworkCapabilities capabilities, int legacyType,
            int requestId, String typeName) {
        Parcel parcel = Parcel.obtain();
        capabilities.writeToParcel(parcel, 0);
        parcel.writeInt(legacyType);
        parcel.writeInt(requestId);
        parcel.writeString(typeName);
        parcel.setDataPosition(0);
        NetworkRequest result = NetworkRequest.CREATOR.createFromParcel(parcel);
        parcel.recycle();
        return result;
    }

    private static final class Registration {
        final NetworkRequest request;
        final Messenger messenger;
        final IBinder callerBinder;
        final boolean tracksDefault;
        IBinder.DeathRecipient deathRecipient;
        boolean dead;
        boolean delivered;

        Registration(NetworkRequest value, Messenger callbackMessenger, IBinder binder,
                boolean followsDefault) {
            request = value;
            messenger = callbackMessenger;
            callerBinder = binder;
            tracksDefault = followsDefault;
        }
    }
}
