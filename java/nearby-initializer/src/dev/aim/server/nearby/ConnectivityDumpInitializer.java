package dev.aim.server.nearby;

import android.content.Context;
import android.net.LinkProperties;
import android.net.Network;
import android.net.connectivity.android.net.shared.PrivateDnsConfig;
import android.net.connectivity.com.android.net.module.util.BpfUtils;
import android.os.Binder;
import android.os.SystemClock;
import android.provider.Settings;
import android.net.connectivity.com.android.internal.util.IndentingPrintWriter;
import android.net.connectivity.com.android.net.module.util.DeviceConfigUtils;
import com.android.server.SystemService;
import android.net.connectivity.com.android.server.connectivity.ConnectivityResources;
import android.net.connectivity.com.android.server.connectivity.KeepaliveResourceUtil;
import android.net.connectivity.com.android.server.connectivity.NetworkDiagnostics;
import android.net.connectivity.com.android.metrics.*;
import dev.aim.server.IConnectivityDumpLeaf;
import java.io.StringWriter;
import java.util.Arrays;

/** Read-only leaves from platform/packages/modules/Connectivity, android-16.0.0_r1:
 * service/src/com/android/server/ConnectivityService.java and connectivity/KeepaliveTracker.java.
 * Original diagnostics, resource/BPF access and protobuf rendering stay in the module loader.
 * Copyright (C) 2008, 2015 The Android Open Source Project, Apache License 2.0.
 */
public final class ConnectivityDumpInitializer extends SystemService {
    public ConnectivityDumpInitializer(Context context) { super(context); }
    @Override public void onStart() {
        publishBinderService("aim.connectivity_dump", new Owner(getContext()), false);
    }
    static final class Owner extends IConnectivityDumpLeaf.Stub {
        private final Context context;
        private final long started = SystemClock.elapsedRealtime();
        private final ConnectivityResources resources;
        Owner(Context context) {
            this.context = context;
            resources = new ConnectivityResources(context);
        }
        private int integer(String name) {
            var r = resources.get();
            return r.getInteger(r.getIdentifier(name, "integer",
                    resources.getResourcesContext().getPackageName()));
        }
        @Override public String dump(int section, int netId, LinkProperties lp, boolean verbose) {
            if (Binder.getCallingUid() != 1000) throw new SecurityException("system uid only");
            StringWriter out = new StringWriter();
            var p = new IndentingPrintWriter(out, "  ");
            switch (section) {
                case DIAGNOSTICS:
                    if (netId < 0) break;
                    var cfg = new PrivateDnsConfig(lp.isPrivateDnsActive());
                    if (lp.getPrivateDnsServerName() != null) cfg = new PrivateDnsConfig(
                            lp.getPrivateDnsServerName(),
                            lp.getValidatedPrivateDnsServers().toArray(java.net.InetAddress[]::new));
                    var diag = new NetworkDiagnostics(new Network(netId), new LinkProperties(lp), cfg, 5000);
                    p.println();
                    diag.waitForMeasurements();
                    diag.dump(p);
                    break;
                case TRAFFIC: ConnectivityDumpMaps.dump(p, verbose); break;
                case KEEPALIVE:
                    p.println("Supported Socket keepalives: " + Arrays.toString(
                            KeepaliveResourceUtil.getSupportedKeepalives(context)));
                    p.println("Reserved Privileged keepalives: " + integer("config_reservedPrivilegedKeepaliveSlots"));
                    p.println("Allowed Unprivileged keepalives per uid: " + integer("config_allowedUnprivilegedKeepalivePerUid"));
                    p.println("Socket keepalives:");
                    p.println("AutomaticOnOff enabled: " + DeviceConfigUtils.isTetheringFeatureNotChickenedOut(
                            context, "automatic_on_off_keepalive_disable_flag"));
                    p.println("Events (most recent first):");
                    p.println();
                    // KeepaliveStatsTracker's empty metrics: no native keepalives (#1509).
                    int duration = (int) (SystemClock.elapsedRealtime() - started);
                    p.println("KeepaliveStatsTracker enabled: true");
                    p.increaseIndent();
                    p.println(DailykeepaliveInfoReported.newBuilder()
                            .setDurationPerNumOfKeepalive(DurationPerNumOfKeepalive.newBuilder()
                                    .addDurationForNumOfKeepalive(DurationForNumOfKeepalive.newBuilder()
                                            .setNumOfKeepalive(0)
                                            .setKeepaliveRegisteredDurationsMsec(duration)
                                            .setKeepaliveActiveDurationsMsec(duration)))
                            .setKeepaliveLifetimePerCarrier(KeepaliveLifetimePerCarrier.newBuilder())
                            .setKeepaliveRequests(0).setAutomaticKeepaliveRequests(0)
                            .setDistinctUserCount(0).build().toString());
                    p.decreaseIndent();
                    break;
                case POLICY:
                    if (integer("config_networkAvoidBadWifi") != 0) {
                        p.println("Bad Wi-Fi avoidance: unrestricted");
                        break;
                    }
                    String setting = Settings.Global.getString(context.getContentResolver(), "network_avoid_bad_wifi");
                    p.println("Bad Wi-Fi avoidance: " + "1".equals(setting));
                    p.increaseIndent();
                    p.println("Config restrict:               true");
                    p.println("Actively prefer bad wifi:      true");
                    p.println("Avoid bad wifi setting:        " + (setting == null ? "prompt"
                            : "0".equals(setting) ? "get stuck" : "1".equals(setting) ? "avoid" : setting + " (?)"));
                    p.println("Actively prefer bad wifi conf: force true");
                    p.println();
                    p.println("Network overrides:");
                    p.decreaseIndent();
                    break;
                case BPF_STATUS:
                    p.println("Bpf Program Status:");
                    p.increaseIndent();
                    String[] names = {"INET_INGRESS", "INET_EGRESS", "INET_SOCK_CREATE",
                            "INET4_BIND", "INET6_BIND", "INET4_CONNECT", "INET6_CONNECT",
                            "UDP4_SENDMSG", "UDP6_SENDMSG", "UDP4_RECVMSG", "UDP6_RECVMSG",
                            "GETSOCKOPT", "SETSOCKOPT", "INET_SOCK_RELEASE"};
                    int[] types = {0, 1, 2, 8, 9, 10, 11, 14, 15, 19, 20, 21, 22, 34};
                    try {
                        for (int i = 0; i < names.length; i++) {
                            p.print("CGROUP_" + names[i] + ": ");
                            p.println(BpfUtils.getProgramId(types[i]));
                        }
                    } catch (java.io.IOException e) { p.println("  IOException"); }
                    p.decreaseIndent();
                    break;
                case ACTIVITY:
                    p.increaseIndent();
                    p.println("mTrackMultiNetworkActivities=true");
                    p.println("mDefaultCellularDataInactivityTimeout=" + DeviceConfigUtils.getDeviceConfigPropertyInt("tethering_boot", "cellular_data_inactivity_timeout", 10));
                    p.println("mDefaultWifiDataInactivityTimeout=" + DeviceConfigUtils.getDeviceConfigPropertyInt("tethering_boot", "wifi_data_inactivity_timeout", 15));
                    break;
                default: throw new IllegalArgumentException("Unknown connectivity dump leaf " + section);
            }
            p.flush();
            return out.toString();
        }
    }
}
