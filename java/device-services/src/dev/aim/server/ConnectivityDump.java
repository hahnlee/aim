package dev.aim.server;

import java.io.PrintWriter;
import java.io.StringWriter;
import java.util.Arrays;
import android.net.NetworkRequest;

/**
 * ConnectivityService.doDump/dumpNetworks/NetworkRequestInfo.toString,
 * android-16.0.0_r1. Original parcelables supply their own strings. Missing
 * owners are errors at their original positions, never invented empty state.
 * Copyright (C) The Android Open Source Project, Apache License 2.0.
 */
final class ConnectivityDump {
    private static void error(PrintWriter p, String indent, String owner) {
        p.println(indent + "Error: " + owner + " owner unavailable in app host (#2333)");
    }
    private static void section(PrintWriter p, String title, String owner) {
        p.println(title);
        error(p, "  ", owner);
    }
    static String format(ConnectivityDumpSnapshot s, String[] args) {
        var options = args == null ? java.util.List.<String>of() : Arrays.asList(args);
        StringWriter out = new StringWriter();
        PrintWriter p = new PrintWriter(out);
        // LocalPriorityDump HIGH emits diagnostics followed by the short dump.
        boolean high = false;
        for (int i = 0; i + 1 < options.size(); i++) {
            if ("--dump-priority".equals(options.get(i))) high = "HIGH".equals(options.get(++i));
        }
        if (high) {
            error(p, "", "NetworkDiagnostics");
            options = java.util.List.of("--short");
        }
        if (options.contains("--diag")) {
            error(p, "", "NetworkDiagnostics");
        } else if (options.contains("networks")) {
            networks(p, s, "");
        } else if (options.contains("requests")) {
            requests(p, s, "");
        } else if (options.contains("trafficcontroller")) {
            error(p, "", "BpfNetMaps/TrafficController");
        } else if (options.contains("clatEgress4RawBpfMap")
                || options.contains("clatIngress6RawBpfMap")) {
            error(p, "", "Nat464Xlat BPF maps");
        } else {
            p.println("NetworkProviders for:");
            p.println("  " + s.providerId + ": " + s.providerName);
            p.println();
            p.println("Active default network: " + (s.netId < 0 ? "none" : s.netId));
            p.println();
            section(p, "Current network preferences: ", "profile/OEM/default network preferences");
            p.println();
            p.println("Current Networks:");
            networks(p, s, "  ");
            p.println();
            p.println("Status for known UIDs:");
            for (int i = 0; i < s.uids.length; i++) {
                p.println("  UID=" + s.uids[i] + " blockedReasons="
                        + Integer.toHexString(s.blockedReasons[i]));
            }
            error(p, "  ", "NetworkPolicyManager firewall/data saver blocked reasons");
            p.println();
            p.println("Network Requests:");
            requests(p, s, "  ");
            p.println();
            p.println("Network Offers:");
            if (s.offerCapabilities != null) {
                // NetworkOffer.toString, from the native provider's current offer.
                p.println("  NetworkOffer [ Provider Id (" + s.providerId + ") "
                        + score(s.offerScorePolicies, s.offerKeepConnectedReason) + " Caps "
                        + s.offerCapabilities + " Needed by " + Arrays.toString(s.offerNeeded) + "]");
            }
            p.println();
            p.println("mLegacyTypeTracker:");
            if (s.legacyTypes == null) {
                p.println("  Error: LegacyTypeTracker feature owner: " + s.legacyError);
            } else {
                p.print("  Supported types:");
                for (int type : s.legacyTypes) p.print(" " + type);
                p.println();
            }
            p.println("  Current state:");
            if (s.netId >= 0) p.println("    9 [" + s.netId + " ETHERNET]");
            p.println();
            p.println();
            section(p, "KeepaliveTracker:", "KeepaliveTracker");
            p.println();
            error(p, "", "MultinetworkPolicyTracker avoid bad Wi-Fi settings");
            p.println();
            error(p, "", "ConnectivityService socket destruction policy");
            p.println();
            error(p, "", "ConnectivityService BPF program status");
            p.println();
            error(p, "", "CarrierPrivilegeAuthenticator");
            p.println();
            if (!options.contains("--short")) {
                p.println();
                section(p, "mNetworkRequestInfoLogs (most recent first):", "request history");
                p.println();
                section(p, "mNetworkInfoBlockingLogs (most recent first):", "blocking history");
                p.println();
                section(p, "NetTransition WakeLock activity (most recent first):", "transition wakelock");
                p.println();
                section(p, "  bandwidth update requests (by uid):", "bandwidth request counters");
                p.println();
                section(p, "mOemNetworkPreferencesLogs (most recent first):", "OEM preference history");
            }
            p.println();
            p.println();
            section(p, "Permission Monitor:", "PermissionMonitor");
            p.println();
            p.println("Legacy network activity:");
            p.println("  mIsDefaultNetworkActive=true");
            error(p, "  ", "NetworkActivityTracker cellular/Wi-Fi idle timers");
            p.println();
            error(p, "", "MulticastRoutingCoordinatorService");
            error(p, "", "ConnectivityService background firewall chain");
            error(p, "", "ConnectivityService ingress-to-VPN filtering");
        }
        p.flush();
        return out.toString();
    }

    private static void networks(PrintWriter p, ConnectivityDumpSnapshot s, String indent) {
        if (s.netId < 0) return;
        // NetworkAgentInfo.toString: creation is wall time, lifecycle events are
        // elapsed realtime. Connected time is carried but the original omits it.
        long handle = ((long) s.netId << 32) | 0xcafed00dL;
        p.println(indent + "NetworkAgentInfo{network{" + s.netId + "}  handle{" + handle
                + "}  ni{" + s.info.toShortString() + "} "
                + "created=" + java.time.Instant.ofEpochMilli(s.creationTime) + " "
                + score(s.scorePolicies, s.keepConnectedReason) + " "
                + (s.createdTime != 0 ? " created " + s.createdTime : "")
                + (s.firstValidationTime != 0 ? " firstValidated " + s.firstValidationTime : "")
                + (s.lastValidationTime != 0 ? " lastValidated " + s.lastValidationTime : "")
                + "  lp{" + s.links + "}  nc{" + s.capabilities + "}"
                + "  factorySerialNumber=" + s.factorySerialNumber + "}");
        String sub = indent + "  ";
        p.println(sub + "Nat464Xlat:");
        error(p, sub + "  ", "Nat464Xlat");
        int foreground = 0, listen = 0, background = 0;
        for (var n : s.requests) {
            if (n.satisfier != s.netId) continue;
            if (n.request.type == NetworkRequest.Type.REQUEST) foreground++;
            else if (n.request.type == NetworkRequest.Type.BACKGROUND_REQUEST) background++;
            else listen++;
        }
        p.println(sub + String.format("Requests: REQUEST:%d LISTEN:%d BACKGROUND_REQUEST:%d total:%d",
                foreground, listen, background, foreground + listen + background));
        for (var n : s.requests) {
            if (n.satisfier == s.netId) p.println(sub + "  " + n.request);
        }
        p.println(sub + "Inactivity Timers:");
        error(p, sub + "  ", "NetworkAgentInfo inactivity timers");
    }

    /** FullScore.toString's policy order and names at android-16.0.0_r1. */
    private static String score(long policies, int keepConnected) {
        var out = new java.util.StringJoiner("&", "Score(Policies : ",
                " ; KeepConnected : " + keepConnected + ")");
        String[] agent = {"YIELD_TO_BAD_WIFI", "TRANSPORT_PRIMARY", "EXITING"};
        String[] service = {"IS_DESTROYED", "EVER_EVALUATED", "IS_INVINCIBLE", "IS_UNMETERED",
                "AVOIDED_WHEN_UNVALIDATED", "ACCEPT_UNVALIDATED", "EVER_USER_SELECTED",
                "IS_VPN", "EVER_VALIDATED", "IS_VALIDATED"};
        for (int i = 0; i < agent.length; i++) {
            if ((policies & (1L << (i + 1))) != 0) out.add(agent[i]);
        }
        for (int i = 0; i < service.length; i++) {
            if ((policies & (1L << (i + 54))) != 0) out.add(service[i]);
        }
        return out.toString();
    }

    private static void requests(PrintWriter p, ConnectivityDumpSnapshot s, String indent) {
        for (var n : s.requests) {
            String asUid = n.asUid == n.uid ? "" : " asUid: " + n.asUid;
            Object active = n.satisfier < 0 || n.request.type == NetworkRequest.Type.LISTEN
                    ? null : n.request.requestId;
            p.println(indent + "uid/pid:" + n.uid + "/" + n.pid + asUid
                    + " activeRequest: " + active + " callbackRequest: " + n.request.requestId
                    + " [" + n.request + "] callback flags: " + n.flags
                    + " order: 2147483647 isUidTracked: " + n.tracked
                    + " declaredMethods: " + declared(n.declared));
        }
    }
    private static String declared(int flags) {
        if (flags == -1) return "ALL";
        if (flags == 0) return "NONE";
        StringBuilder out = new StringBuilder();
        String[] names = {"PRECHK", "AVAIL", "LOSING", "LOST", "UNAVAIL", "NC", "LP",
                null, "SUSP", "RESUME", "BLK", "LOCALINF", "RES"};
        for (int i = 0; i < names.length; i++) {
            if (names[i] == null) continue;
            int mask = 1 << (i + 1);
            if ((flags & mask) == 0) continue;
            if (out.length() > 0) out.append('|');
            out.append(names[i]); flags &= ~mask;
        }
        if (flags != 0) out.append("|0x").append(Integer.toHexString(flags));
        return out.toString();
    }
}
