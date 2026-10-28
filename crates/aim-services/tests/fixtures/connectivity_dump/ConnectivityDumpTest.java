package dev.aim.server;

import android.net.*;
import java.nio.file.*;

public final class ConnectivityDumpTest {
    public static void main(String[] args) throws Exception {
        NetworkInfo.text = "Ethernet CONNECTED extra: 02:61:69:6d:00:02";
        LinkProperties.text = "{InterfaceName: eth0 LinkAddresses: [ 192.0.2.2/24 ] DnsAddresses: [ /192.0.2.1 ] MTU: 1500}";
        NetworkCapabilities.text = "[ Transports: ETHERNET Capabilities: INTERNET&VALIDATED&FOREGROUND ]";
        NetworkRequest.text = "NetworkRequest [ REQUEST id=7, [ Capabilities: INTERNET ] ]";
        var s = new ConnectivityDumpSnapshot();
        s.netId = 100;
        s.info = new NetworkInfo(9, -1, "Ethernet", "");
        s.links = new LinkProperties(); s.capabilities = new NetworkCapabilities();
        s.providerId = 1; s.providerName = "EthernetNetworkFactory";
        s.offerCapabilities = new NetworkCapabilities();
        s.offerCapabilities.value = "[ Transports: ETHERNET Capabilities: INTERNET ]";
        s.offerScorePolicies = s.scorePolicies = (1L << 55) | (1L << 57) | (1L << 62) | (1L << 63);
        s.offerNeeded = new int[]{7}; s.factorySerialNumber = s.providerId;
        s.creationTime = 1760000000123L; s.createdTime = 1200; s.connectedTime = 1100;
        s.firstValidationTime = 1150; s.lastValidationTime = 1250;
        var n = new ConnectivityDumpRequest();
        n.request = new NetworkRequest(null); n.uid = 10100; n.pid = 42; n.asUid = 10100;
        n.declared = -1; n.satisfier = 100; n.tracked = true;
        s.requests = new ConnectivityDumpRequest[]{n};
        s.uids = new int[]{10100}; s.blockedReasons = new int[]{256};
        s.legacyTypes = new int[]{9, 17};
        var dir = Path.of(args[0]);
        String shortDump = Files.readString(dir.resolve("short.txt"));
        equal(shortDump, ConnectivityDump.format(s, new String[]{"--short"}));
        String networks = Files.readString(dir.resolve("networks.txt"));
        equal(networks, ConnectivityDump.format(s, new String[]{"networks", "--short"}));
        String request = Files.readString(dir.resolve("requests.txt"));
        equal(request, ConnectivityDump.format(s, new String[]{"requests", "-a"}));
        equal("Error: BpfNetMaps/TrafficController owner unavailable in app host (#2333)\n",
                ConnectivityDump.format(s, new String[]{"trafficcontroller", "--short"}));
        String full = Files.readString(dir.resolve("full.txt"));
        equal(full, ConnectivityDump.format(s, new String[]{"-a"}));
        equal(full, ConnectivityDump.format(s, null));
        // NAI omits connected time and zero lifecycle events; factory identity
        // and score are carried by the snapshot, not reconstructed by formatting.
        s.createdTime = 0; s.firstValidationTime = 0; s.lastValidationTime = 0;
        s.scorePolicies = (1L << 1) | (1L << 2) | (1L << 3) | (1L << 54) | (1L << 56)
                | (1L << 58) | (1L << 59) | (1L << 60) | (1L << 61);
        s.keepConnectedReason = 2; s.factorySerialNumber = 8;
        equal(networks.replace(" created 1200 firstValidated 1150 lastValidated 1250", "")
                .replace("EVER_EVALUATED&IS_UNMETERED&EVER_VALIDATED&IS_VALIDATED",
                        "YIELD_TO_BAD_WIFI&TRANSPORT_PRIMARY&EXITING&IS_DESTROYED&IS_INVINCIBLE"
                        + "&AVOIDED_WHEN_UNVALIDATED&ACCEPT_UNVALIDATED&EVER_USER_SELECTED&IS_VPN")
                .replace("KeepConnected : 0", "KeepConnected : 2")
                .replace("factorySerialNumber=1", "factorySerialNumber=8"),
                ConnectivityDump.format(s, new String[]{"networks"}));
        s.createdTime = 1200; s.firstValidationTime = 1150; s.lastValidationTime = 1250;
        s.scorePolicies = s.offerScorePolicies; s.keepConnectedReason = 0; s.factorySerialNumber = 1;
        equal("Error: NetworkDiagnostics owner unavailable in app host (#2333)\n" + shortDump,
                ConnectivityDump.format(s, new String[]{"--dump-priority", "HIGH"}));
        n.asUid = 10101; n.declared = (1 << 9) | (1 << 13) | (1 << 8);
        equal(request.replace("10100/42", "10100/42 asUid: 10101")
                .replace("ALL", "SUSP|RES|0x100"), ConnectivityDump.format(s, new String[]{"requests"}));
        s.netId = -1;
        equal("", ConnectivityDump.format(s, new String[]{"networks"}));
        n.asUid = n.uid; n.declared = -1; n.satisfier = -1;
        s.offerCapabilities = null; s.offerNeeded = new int[0];
        equal(Files.readString(dir.resolve("down.txt")),
                ConnectivityDump.format(s, new String[]{"--short"}));
    }
    private static void equal(String expected, String actual) {
        if (!expected.equals(actual)) throw new AssertionError("expected:\n" + expected + "actual:\n" + actual);
    }
}
