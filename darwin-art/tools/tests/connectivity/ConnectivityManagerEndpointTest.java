package dev.darwinart.runtime.connectivity;

import android.Manifest;
import android.os.IBinder;
import android.os.Message;
import android.os.Messenger;
import android.os.Parcel;
import android.os.RemoteException;
import android.net.NetworkCapabilities;
import android.net.Network;
import android.net.NetworkRequest;
import java.util.ArrayList;
import java.util.List;
import java.net.InetAddress;

/** Focused host-Java contract tests for the narrow connectivity Binder endpoint. */
public final class ConnectivityManagerEndpointTest {
    private static final class TestPermissions implements ConnectivityPermissionEnforcer {
        boolean allowed = true;
        int checks;

        @Override
        public void enforceAccessNetworkState() {
            checks++;
            if (!allowed) throw new SecurityException("denied");
        }
    }

    private static final class TestState implements ConnectivityState {
        boolean metered;
        int reads;
        android.net.ProxyInfo proxy;

        @Override
        public android.net.ProxyInfo activeNetworkProxy() {
            return proxy;
        }

        TestState(boolean value) {
            metered = value;
        }

        @Override
        public boolean isActiveNetworkMetered() {
            reads++;
            return metered;
        }
    }

    private static final class SnapshotState implements ConnectivityState {
        ConnectivitySnapshot value;
        int reads;

        SnapshotState(ConnectivitySnapshot snapshot) { value = snapshot; }

        @Override
        public boolean isActiveNetworkMetered() { return value.isMetered(); }

        @Override
        public ConnectivitySnapshot snapshot() { reads++; return value; }
    }

    private static final class DeathBinder implements IBinder {
        DeathRecipient recipient;
        boolean unlinked;

        @Override public boolean transact(int code, Parcel data, Parcel reply, int flags) {
            return false;
        }
        @Override public void linkToDeath(DeathRecipient value, int flags) {
            recipient = value;
        }
        @Override public boolean unlinkToDeath(DeathRecipient value, int flags) {
            unlinked = recipient == value;
            return unlinked;
        }
        void die() { if (recipient != null) recipient.binderDied(); }
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static Parcel request() {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(ConnectivityManagerEndpoint.DESCRIPTOR);
        return data;
    }

    private static Parcel callbackRequest(NetworkCapabilities capabilities, Messenger messenger,
            IBinder binder) {
        return callbackRequest(capabilities, messenger, binder, 3);
    }

    private static Parcel callbackRequest(NetworkCapabilities capabilities, Messenger messenger,
            IBinder binder, int requestType) {
        Parcel data = request();
        data.writeInt(10001); // uid
        data.writeTypedObject(capabilities, 0);
        data.writeInt(requestType);
        data.writeTypedObject(messenger, 0);
        data.writeInt(0); // timeoutSec
        data.writeStrongBinder(binder);
        data.writeInt(-1); // legacyType
        data.writeInt(0); // callbackFlags
        data.writeString("test.package");
        data.writeString(null);
        data.writeInt(0); // declaredMethodsFlag
        return data;
    }

    private static void testCallbackLifecycle() throws Exception {
        final List<Message> messages = new ArrayList<Message>();
        Messenger messenger = new Messenger(new Messenger.Sender() {
            @Override public void send(Message message) throws RemoteException {
                messages.add(message);
            }
        });
        DeathBinder binder = new DeathBinder();
        ConnectivityStateOwner state = new ConnectivityStateOwner();
        state.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(true, false, false, 1));
        ConnectivityCallbackRegistry registry = new ConnectivityCallbackRegistry(state);
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(
                new TestPermissions(), state, registry);

        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_REQUEST_NETWORK,
                callbackRequest(new NetworkCapabilities(), messenger, binder), reply, 0),
                "requestNetwork callback transaction returned false");
        NetworkRequest request = reply.readTypedObject(NetworkRequest.CREATOR);
        check(request != null && request.getRequestId() == 1,
                "system_server did not allocate request id");
        check(messages.size() == 1, "initial callback sequence length changed");
        check(messages.get(0).what == ConnectivityCallbackRegistry.CALLBACK_AVAILABLE,
                "initial callback type changed");
        for (Message message : messages) {
            check(message.getData().getParcelable("NetworkRequest").equals(request),
                    "callback omitted registered request identity");
            check(message.getData().getParcelable("Network") != null,
                    "callback omitted active network");
        }
        check(messages.get(0).getData().getParcelable("NetworkCapabilities") != null,
                "capabilities callback omitted capabilities");
        check(messages.get(0).getData().getParcelable("LinkProperties") != null,
                "link-properties callback omitted link properties");

        registry.updateSnapshot(ConnectivitySnapshot.unavailable());
        check(messages.get(1).what == ConnectivityCallbackRegistry.CALLBACK_LOST,
                "network loss did not deliver onLost");
        registry.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(true, false, false, 1));
        check(messages.get(2).what == ConnectivityCallbackRegistry.CALLBACK_AVAILABLE,
                "new network incarnation did not deliver onAvailable");
        Network first = messages.get(0).getData().getParcelable("Network");
        Network second = messages.get(2).getData().getParcelable("Network");
        check(!first.equals(second), "active network incarnation was reused");

        Parcel release = request();
        release.writeTypedObject(request, 0);
        Parcel releaseReply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_RELEASE_NETWORK_REQUEST,
                release, releaseReply, 0), "releaseNetworkRequest returned false");
        check(registry.registrationCount() == 0 && binder.unlinked,
                "release did not unlink caller death recipient");
    }

    private static void testDefaultCallbackAllowsNullCapabilities() throws Exception {
        final List<Message> messages = new ArrayList<Message>();
        Messenger messenger = new Messenger(new Messenger.Sender() {
            @Override public void send(Message message) throws RemoteException {
                messages.add(message);
            }
        });
        ConnectivityStateOwner state = new ConnectivityStateOwner();
        state.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(true, false, false, 1));
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(
                new TestPermissions(), state);

        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_REQUEST_NETWORK,
                callbackRequest(null, messenger, new DeathBinder(), 2), reply, 0),
                "TRACK_DEFAULT with null capabilities returned false");
        NetworkRequest request = reply.readTypedObject(NetworkRequest.CREATOR);
        check(request != null && request.getRequestId() == 1,
                "TRACK_DEFAULT did not acquire a framework request");
        check(messages.size() == 1 && messages.get(0).what
                        == ConnectivityCallbackRegistry.CALLBACK_AVAILABLE,
                "TRACK_DEFAULT did not receive the initial network lifecycle");
    }

    private static void testPerRegistrationCapabilityTransitions() throws Exception {
        final List<Message> messages = new ArrayList<Message>();
        Messenger messenger = new Messenger(new Messenger.Sender() {
            @Override public void send(Message message) throws RemoteException {
                messages.add(message);
            }
        });
        ConnectivityStateOwner state = new ConnectivityStateOwner();
        ConnectivityCallbackRegistry registry = new ConnectivityCallbackRegistry(state);
        NetworkCapabilities requiresUnmetered = new NetworkCapabilities().addCapability(
                NetworkCapabilities.NET_CAPABILITY_NOT_METERED);
        ConnectivitySnapshot metered = ConnectivitySnapshot.fromNetworkPath(true, true, false, 1);
        state.updateSnapshot(metered);
        registry.updateSnapshot(metered);
        NetworkRequest request = registry.requestNetwork(10001, requiresUnmetered, 3, messenger,
                0, new DeathBinder(), -1, 0, "test.package", null, 0);
        check(messages.isEmpty(), "unmatched registration received initial onAvailable");

        registry.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(true, false, false, 1));
        check(messages.size() == 1 && messages.get(0).what
                        == ConnectivityCallbackRegistry.CALLBACK_AVAILABLE,
                "registration becoming matched did not receive onAvailable");
        registry.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(true, true, false, 1));
        check(messages.size() == 2 && messages.get(1).what
                        == ConnectivityCallbackRegistry.CALLBACK_LOST,
                "registration becoming unmatched did not receive onLost");

        registry.updateSnapshot(ConnectivitySnapshot.unavailable());
        check(messages.size() == 2,
                "physical loss delivered onLost to a registration that was not available");
        registry.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(true, false, false, 1));
        check(messages.size() == 3 && messages.get(2).what
                        == ConnectivityCallbackRegistry.CALLBACK_AVAILABLE,
                "registration did not recover after a later matching network");
        registry.releaseNetworkRequest(request);
        registry.updateSnapshot(ConnectivitySnapshot.unavailable());
        check(messages.size() == 3, "released registration received a callback");
    }

    private static void testDefaultTrackerStaysAvailableAcrossCapabilityChanges()
            throws Exception {
        final List<Message> messages = new ArrayList<Message>();
        Messenger messenger = new Messenger(new Messenger.Sender() {
            @Override public void send(Message message) throws RemoteException {
                messages.add(message);
            }
        });
        ConnectivityStateOwner state = new ConnectivityStateOwner();
        ConnectivityCallbackRegistry registry = new ConnectivityCallbackRegistry(state);
        ConnectivitySnapshot unmetered = ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1);
        state.updateSnapshot(unmetered);
        registry.updateSnapshot(unmetered);
        NetworkRequest request = registry.requestNetwork(10001, null, 2, messenger, 0,
                new DeathBinder(), -1, 0, "test.package", null, 0);
        check(messages.size() == 1 && messages.get(0).what
                        == ConnectivityCallbackRegistry.CALLBACK_AVAILABLE,
                "TRACK_DEFAULT did not become available");

        registry.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(true, false, false, 1)
                .withValidation(true));
        check(messages.size() == 3
                        && messages.get(1).what == ConnectivityCallbackRegistry.CALLBACK_CAP_CHANGED
                        && messages.get(2).what == ConnectivityCallbackRegistry.CALLBACK_IP_CHANGED,
                "TRACK_DEFAULT was treated as capability-unmatched");
        registry.updateSnapshot(ConnectivitySnapshot.unavailable());
        check(messages.size() == 4 && messages.get(3).what
                        == ConnectivityCallbackRegistry.CALLBACK_LOST,
                "TRACK_DEFAULT did not receive physical onLost");
        registry.releaseNetworkRequest(request);
    }

    private static void testValidationCapabilityTransitions() throws Exception {
        final List<Message> messages = new ArrayList<Message>();
        Messenger messenger = new Messenger(new Messenger.Sender() {
            @Override public void send(Message message) throws RemoteException {
                messages.add(message);
            }
        });
        ConnectivityStateOwner state = new ConnectivityStateOwner();
        ConnectivityCallbackRegistry registry = new ConnectivityCallbackRegistry(state);
        ConnectivitySnapshot unvalidated = ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1).withValidation(false);
        state.updateSnapshot(unvalidated);
        registry.updateSnapshot(unvalidated);
        NetworkCapabilities requiresValidation = new NetworkCapabilities().addCapability(
                NetworkCapabilities.NET_CAPABILITY_VALIDATED);
        NetworkRequest request = registry.requestNetwork(10001, requiresValidation, 3, messenger,
                0, new DeathBinder(), -1, 0, "test.package", null, 0);
        check(messages.isEmpty(), "unvalidated registration received initial onAvailable");

        ConnectivitySnapshot validated = ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1).withValidation(true);
        registry.updateSnapshot(validated);
        check(messages.size() == 1 && messages.get(0).what
                        == ConnectivityCallbackRegistry.CALLBACK_AVAILABLE,
                "validated registration did not receive onAvailable");
        registry.updateSnapshot(unvalidated);
        check(messages.size() == 2 && messages.get(1).what
                        == ConnectivityCallbackRegistry.CALLBACK_LOST,
                "validated registration did not receive onLost when validation was lost");
        registry.updateSnapshot(ConnectivitySnapshot.unavailable());
        check(messages.size() == 2,
                "unvalidated registration received a duplicate physical-loss onLost");
        registry.releaseNetworkRequest(request);
    }

    private static void testBinderDeathSuppressesCallbacks() throws Exception {
        final List<Message> messages = new ArrayList<Message>();
        Messenger messenger = new Messenger(new Messenger.Sender() {
            @Override public void send(Message message) throws RemoteException {
                messages.add(message);
            }
        });
        ConnectivityStateOwner state = new ConnectivityStateOwner();
        ConnectivityCallbackRegistry registry = new ConnectivityCallbackRegistry(state);
        ConnectivitySnapshot unmetered = ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1);
        state.updateSnapshot(unmetered);
        registry.updateSnapshot(unmetered);
        DeathBinder binder = new DeathBinder();
        registry.requestNetwork(10001, new NetworkCapabilities(), 3, messenger, 0, binder,
                -1, 0, "test.package", null, 0);
        check(messages.size() == 1, "binder-death test did not receive onAvailable");
        binder.die();
        registry.updateSnapshot(ConnectivitySnapshot.unavailable());
        check(registry.registrationCount() == 0 && messages.size() == 1,
                "dead registration received a callback");
    }

    private static void testConservativeStateOwner() {
        ConnectivityStateOwner owner = new ConnectivityStateOwner();
        check(owner.isActiveNetworkMetered(), "initial state was not conservative");
        owner.updateActiveNetwork(true, false);
        check(!owner.isActiveNetworkMetered(), "active unmetered network lost");
        owner.updateActiveNetwork(true, true);
        check(owner.isActiveNetworkMetered(), "active metered network lost");
        owner.updateActiveNetwork(false, false);
        check(owner.isActiveNetworkMetered(), "no-active state became unmetered");
        owner.updateActiveNetwork(true, false);
        owner.clearActiveNetwork();
        check(owner.isActiveNetworkMetered(), "cleared state was not conservative");
    }

    private static void testSnapshotCopiesLinkFactsAndGeneration() {
        String[] dns = new String[] {"1.1.1.1", "2606:4700:4700::1111"};
        ConnectivitySnapshot snapshot = ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1, 7, "en0", dns);
        dns[0] = "9.9.9.9";
        check(snapshot.generation() == 7, "snapshot generation was not retained");
        check("en0".equals(snapshot.interfaceName()), "interface name was not retained");
        check("1.1.1.1".equals(snapshot.dnsServers()[0]),
                "snapshot did not defensively copy DNS facts");
        String[] returned = snapshot.dnsServers();
        returned[1] = "9.9.9.9";
        check("2606:4700:4700::1111".equals(snapshot.dnsServers()[1]),
                "snapshot DNS accessor leaked mutable storage");
    }

    private static void testDnsOnlyChangeIsObservable() throws Exception {
        final List<Message> messages = new ArrayList<Message>();
        Messenger messenger = new Messenger(new Messenger.Sender() {
            @Override public void send(Message message) throws RemoteException {
                messages.add(message);
            }
        });
        ConnectivityStateOwner state = new ConnectivityStateOwner();
        ConnectivityCallbackRegistry registry = new ConnectivityCallbackRegistry(state);
        state.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1, 1, "en0", new String[] {"1.1.1.1"}));
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(
                new TestPermissions(), state, registry);
        Parcel reply = Parcel.obtain();
        endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_REQUEST_NETWORK,
                callbackRequest(new NetworkCapabilities(), messenger, new DeathBinder()), reply, 0);
        check(messages.size() == 1, "DNS test did not receive initial callback");
        registry.updateSnapshot(ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1, 2, "en0", new String[] {"9.9.9.9"}));
        check(messages.size() == 3, "DNS-only change did not emit capability and link callbacks");
        check(messages.get(1).what == ConnectivityCallbackRegistry.CALLBACK_CAP_CHANGED
                        && messages.get(2).what == ConnectivityCallbackRegistry.CALLBACK_IP_CHANGED,
                "DNS-only callback ordering changed");
    }

    private static void testLinkPropertiesProjection() throws Exception {
        ConnectivitySnapshot snapshot = ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1, 3, "en0",
                new String[] {"1.1.1.1", "2606:4700:4700::1111"});
        ConnectivityProjection projection = new ConnectivityProjection();
        android.net.LinkProperties link = projection.linkProperties(snapshot,
                projection.activeNetwork(snapshot));
        check(link != null && "en0".equals(link.getInterfaceName()),
                "LinkProperties omitted truthful interface name");
        List<InetAddress> dns = link.getDnsServers();
        check(dns.size() == 2 && "1.1.1.1".equals(dns.get(0).getHostAddress()),
                "LinkProperties omitted IPv4 DNS");
        check(dns.get(1).getHostAddress().contains("2606:4700:4700"),
                "LinkProperties omitted IPv6 DNS");
    }

    private static void testValidationProjectionAndOwner() throws Exception {
        check(HttpNetworkProbeTransport.classifyResponse(204)
                        == NetworkProbeTransport.Result.VALIDATED,
                "exact generate-204 response was not validated");
        check(HttpNetworkProbeTransport.classifyResponse(302)
                        == NetworkProbeTransport.Result.CAPTIVE_PORTAL,
                "redirect was not classified as captive portal");
        check(HttpNetworkProbeTransport.classifyResponse(200)
                        == NetworkProbeTransport.Result.FAILED,
                "arbitrary successful response fabricated validation");
        ConnectivitySnapshot base = ConnectivitySnapshot.fromNetworkPath(
                true, false, false, 1, 11, "en0", new String[] {"1.1.1.1"});
        ConnectivityProjection projection = new ConnectivityProjection();
        Network network = projection.activeNetwork(base);
        check(!projection.networkCapabilities(base, network).hasCapability(
                        NetworkCapabilities.NET_CAPABILITY_VALIDATED),
                "host path satisfaction fabricated Android validation");
        check(projection.networkCapabilities(base.withValidation(true), network).hasCapability(
                        NetworkCapabilities.NET_CAPABILITY_VALIDATED),
                "Android validation result was omitted from capabilities");

        SnapshotState host = new SnapshotState(base);
        ConnectivityServiceState state = new ConnectivityServiceState(host,
                new NetworkProbeTransport() {
                    @Override public Result probe() { return Result.VALIDATED; }
                });
        long deadline = System.nanoTime() + 2000000000L;
        while (!state.snapshot().isValidated() && System.nanoTime() < deadline) {
            Thread.sleep(5);
        }
        check(state.snapshot().isValidated(),
                "generation-bound validation result was not published");
        state.close();

        ConnectivityServiceState failed = new ConnectivityServiceState(host,
                new NetworkProbeTransport() {
                    @Override public Result probe() { return Result.FAILED; }
                });
        Thread.sleep(25);
        check(!failed.snapshot().isValidated(), "failed probe fabricated validation");
        failed.close();
    }

    private static void testBooleanReplyAndPermission() throws Exception {
        TestPermissions context = new TestPermissions();
        TestState state = new TestState(false);
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(context, state);
        Parcel reply = Parcel.obtain();

        check(endpoint.onTransact(
                        ConnectivityManagerEndpoint.TRANSACTION_IS_ACTIVE_NETWORK_METERED,
                        request(), reply, 0),
                "supported transaction returned false");
        check(context.checks == 1, "ACCESS_NETWORK_STATE was not enforced once");
        check(state.reads == 1, "connectivity state was not read once");
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        check(!reply.readBoolean(), "boolean reply did not preserve provider fact");
        check(reply.dataAvail() == 0, "boolean reply contained trailing data");
    }

    private static void testProjectedQuerySlice() throws Exception {
        TestPermissions context = new TestPermissions();
        SnapshotState state = new SnapshotState(
                ConnectivitySnapshot.fromNetworkPath(true, false, false, 1));
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(context, state);

        Parcel activeReply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_ACTIVE_NETWORK,
                request(), activeReply, 0), "getActiveNetwork returned false");
        check(activeReply.hasNoException(), "active network omitted writeNoException");
        android.net.Network network = activeReply.readTypedObject(android.net.Network.CREATOR);
        check(network != null && network.getNetId() == ConnectivityProjection.ANDROID_NETWORK_ID,
                "active network identity changed");

        Parcel allReply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_ALL_NETWORKS,
                request(), allReply, 0), "getAllNetworks returned false");
        android.net.Network[] networks = allReply.createTypedArray(android.net.Network.CREATOR);
        check(networks.length == 1 && networks[0].equals(network),
                "all networks did not preserve active identity");

        Parcel infoReply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_ACTIVE_NETWORK_INFO,
                request(), infoReply, 0), "getActiveNetworkInfo returned false");
        android.net.NetworkInfo info =
                infoReply.readTypedObject(android.net.NetworkInfo.CREATOR);
        check(info != null && info.isConnected() && info.getType() == 1,
                "Wi-Fi NetworkInfo projection changed");

        Parcel capabilitiesRequest = request();
        capabilitiesRequest.writeTypedObject(network, 0);
        capabilitiesRequest.writeString("test.package");
        capabilitiesRequest.writeString(null);
        Parcel capabilitiesReply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_NETWORK_CAPABILITIES,
                capabilitiesRequest, capabilitiesReply, 0), "getNetworkCapabilities returned false");
        android.net.NetworkCapabilities capabilities = capabilitiesReply.readTypedObject(
                android.net.NetworkCapabilities.CREATOR);
        check(capabilities != null
                        && capabilities.hasCapability(
                                android.net.NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
                        && capabilities.hasTransport(
                                android.net.NetworkCapabilities.TRANSPORT_WIFI),
                "network capability projection lost host facts");

        Parcel linkRequest = request();
        linkRequest.writeTypedObject(network, 0);
        Parcel linkReply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_LINK_PROPERTIES,
                linkRequest, linkReply, 0), "getLinkProperties returned false");
        check(linkReply.readTypedObject(android.net.LinkProperties.CREATOR) != null,
                "known network omitted LinkProperties");
        check(context.checks == 5 && state.reads == 5,
                "query slice did not enforce/read exactly once per call");
    }

    private static void testUnavailableProjection() throws Exception {
        SnapshotState state = new SnapshotState(ConnectivitySnapshot.unavailable());
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(
                new TestPermissions(), state);
        Parcel activeReply = Parcel.obtain();
        endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_ACTIVE_NETWORK,
                request(), activeReply, 0);
        check(activeReply.readTypedObject(android.net.Network.CREATOR) == null,
                "unavailable path published a Network");
        Parcel allReply = Parcel.obtain();
        endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_ALL_NETWORKS,
                request(), allReply, 0);
        check(allReply.createTypedArray(android.net.Network.CREATOR).length == 0,
                "unavailable path published network list entries");
    }

    private static void testPermissionDeniedBeforeStateRead() throws Exception {
        TestPermissions context = new TestPermissions();
        context.allowed = false;
        TestState state = new TestState(false);
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(context, state);
        try {
            endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_IS_ACTIVE_NETWORK_METERED,
                    request(), Parcel.obtain(), 0);
            throw new AssertionError("permission denial was accepted");
        } catch (SecurityException expected) {
            check(state.reads == 0, "state was read before permission enforcement");
        }
    }

    private static void testTokenAndNoArguments() throws Exception {
        TestPermissions context = new TestPermissions();
        TestState state = new TestState(true);
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(context, state);

        Parcel wrongToken = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.android.net.IConnectivityManager");
        try {
            endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_IS_ACTIVE_NETWORK_METERED,
                    wrongToken, Parcel.obtain(), 0);
            throw new AssertionError("wrong interface token was accepted");
        } catch (SecurityException expected) {
            check(context.checks == 0, "permission checked before interface token");
        }

        Parcel trailing = request();
        trailing.writeInt(7);
        try {
            endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_IS_ACTIVE_NETWORK_METERED,
                    trailing, Parcel.obtain(), 0);
            throw new AssertionError("unexpected argument was accepted");
        } catch (IllegalStateException expected) {
            check(context.checks == 0, "permission checked before enforceNoDataAvail");
            check(state.reads == 0, "state read despite trailing transaction data");
        }
    }

    private static void testInterfaceAndUnsupportedTransactions() throws Exception {
        TestPermissions context = new TestPermissions();
        TestState state = new TestState(true);
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(context, state);
        Parcel descriptorReply = Parcel.obtain();

        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION,
                Parcel.obtain(), descriptorReply, 0), "interface transaction returned false");
        check(ConnectivityManagerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "interface descriptor reply changed");

        check(!endpoint.onTransact(
                        ConnectivityManagerEndpoint.TRANSACTION_IS_ACTIVE_NETWORK_METERED + 1,
                        request(), Parcel.obtain(), 0),
                "unsupported transaction did not preserve Binder false semantics");
        check(context.checks == 0, "unsupported transaction enforced call permission");
        check(state.reads == 0, "unsupported transaction read connectivity state");
    }

    private static void testProxyReplies() throws Exception {
        TestPermissions context = new TestPermissions();
        TestState state = new TestState(false);
        state.proxy = new android.net.ProxyInfo("proxy.example", 3128);
        ConnectivityManagerEndpoint endpoint = new ConnectivityManagerEndpoint(context, state);
        Parcel data = request();
        data.writeTypedObject(new Network(100), 0);
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_PROXY_FOR_NETWORK,
                data, reply, 0), "getProxyForNetwork returned false");
        check(reply.hasNoException(), "getProxyForNetwork omitted writeNoException");
        android.net.ProxyInfo proxy = reply.readTypedObject(android.net.ProxyInfo.CREATOR);
        check(proxy != null && "proxy.example".equals(proxy.getHost()) && proxy.getPort() == 3128,
                "network proxy was not the host provider's");
        Parcel global = Parcel.obtain();
        check(endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_GLOBAL_PROXY,
                request(), global, 0), "getGlobalProxy returned false");
        check(global.hasNoException() && global.readTypedObject(android.net.ProxyInfo.CREATOR) == null,
                "a global proxy override was reported");
        state.proxy = null;
        Parcel none = request();
        none.writeTypedObject(new Network(100), 0);
        Parcel noneReply = Parcel.obtain();
        endpoint.onTransact(ConnectivityManagerEndpoint.TRANSACTION_GET_PROXY_FOR_NETWORK,
                none, noneReply, 0);
        check(noneReply.hasNoException()
                && noneReply.readTypedObject(android.net.ProxyInfo.CREATOR) == null,
                "a direct network reported a proxy");
    }

    private static void testConstructorRequirements() {
        TestPermissions context = new TestPermissions();
        TestState state = new TestState(true);
        try {
            new ConnectivityManagerEndpoint(null, state);
            throw new AssertionError("null Context accepted");
        } catch (NullPointerException expected) {}
        try {
            new ConnectivityManagerEndpoint(context, null);
            throw new AssertionError("null ConnectivityState accepted");
        } catch (NullPointerException expected) {}
    }

    public static void main(String[] args) throws Exception {
        check(ConnectivityManagerEndpoint.TRANSACTION_IS_ACTIVE_NETWORK_METERED == 20,
                "Android 16 transaction code changed");
        check(ConnectivityManagerEndpoint.TRANSACTION_GET_ACTIVE_NETWORK == 1,
                "getActiveNetwork transaction changed");
        check(ConnectivityManagerEndpoint.TRANSACTION_GET_ACTIVE_NETWORK_INFO == 3,
                "getActiveNetworkInfo transaction changed");
        check(ConnectivityManagerEndpoint.TRANSACTION_GET_ALL_NETWORKS == 9,
                "getAllNetworks transaction changed");
        check(ConnectivityManagerEndpoint.TRANSACTION_GET_LINK_PROPERTIES == 14,
                "getLinkProperties transaction changed");
        check(ConnectivityManagerEndpoint.TRANSACTION_GET_NETWORK_CAPABILITIES == 16,
                "getNetworkCapabilities transaction changed");
        check(ConnectivityManagerEndpoint.TRANSACTION_REQUEST_NETWORK == 42,
                "requestNetwork transaction changed");
        check(ConnectivityManagerEndpoint.TRANSACTION_LISTEN_FOR_NETWORK == 45,
                "listenForNetwork transaction changed");
        check(ConnectivityManagerEndpoint.TRANSACTION_RELEASE_NETWORK_REQUEST == 47,
                "releaseNetworkRequest transaction changed");
        testConservativeStateOwner();
        testSnapshotCopiesLinkFactsAndGeneration();
        testDnsOnlyChangeIsObservable();
        testLinkPropertiesProjection();
        testValidationProjectionAndOwner();
        testBooleanReplyAndPermission();
        testProjectedQuerySlice();
        testUnavailableProjection();
        testPermissionDeniedBeforeStateRead();
        testTokenAndNoArguments();
        testInterfaceAndUnsupportedTransactions();
        testProxyReplies();
        testConstructorRequirements();
        testCallbackLifecycle();
        testDefaultCallbackAllowsNullCapabilities();
        testPerRegistrationCapabilityTransitions();
        testDefaultTrackerStaysAvailableAcrossCapabilityChanges();
        testValidationCapabilityTransitions();
        testBinderDeathSuppressesCallbacks();
        System.out.println("connectivity-endpoint: PASS");
    }
}
