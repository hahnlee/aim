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
        equal("Error: NetworkDiagnostics owner unavailable in app host (#2333)\n" + shortDump,
                ConnectivityDump.format(s, new String[]{"--dump-priority", "HIGH"}));
        n.asUid = 10101; n.declared = (1 << 9) | (1 << 13) | (1 << 8);
        equal(request.replace("10100/42", "10100/42 asUid: 10101")
                .replace("ALL", "SUSP|RES|0x100"), ConnectivityDump.format(s, new String[]{"requests"}));
        s.netId = -1;
        equal("", ConnectivityDump.format(s, new String[]{"networks"}));
    }
    private static void equal(String expected, String actual) {
        if (!expected.equals(actual)) throw new AssertionError("expected:\n" + expected + "actual:\n" + actual);
    }
}
