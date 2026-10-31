package dev.aim.server.nearby;

import static android.net.connectivity.android.net.BpfNetMapsConstants.*;
import static android.net.connectivity.android.net.BpfNetMapsUtils.matchToString;
import android.net.connectivity.android.net.UidOwnerValue;
import android.system.ErrnoException;
import android.system.Os;
import android.net.connectivity.android.util.IndentingPrintWriter;
import android.net.connectivity.com.android.net.module.util.*;
import android.net.connectivity.com.android.net.module.util.Struct.*;
import android.net.connectivity.com.android.net.module.util.bpf.*;

/** platform/packages/modules/Connectivity, android-16.0.0_r1:
 * service/src/com/android/server/BpfNetMaps.java:dump, without its state-resetting constructor.
 * Copyright (C) 2022 The Android Open Source Project, Apache License 2.0.
 */
final class ConnectivityDumpMaps {
    private static <K extends Struct, V extends Struct> BpfMap<K,V> map(
            String path, Class<K> key, Class<V> value) throws ErrnoException {
        return new BpfMap<>(path, BpfMap.BPF_F_RDONLY, key, value);
    }
    private static <K extends Struct, V extends Struct> void dumpMap(IndentingPrintWriter p,
            String name, String path, Class<K> key, Class<V> value,
            java.util.function.BiFunction<K,V,String> render) {
        try { BpfDump.dumpMap(map(path, key, value), p, name, render); }
        catch (ErrnoException e) {
            p.println(name + ":");
            p.println("Map dump end with error: " + Os.strerror(e.errno));
        }
    }
    static void dump(IndentingPrintWriter p, boolean verbose) {
        p.println("TrafficController");
        p.println();
        if (!verbose) return;
        p.println();
        p.println("BPF map content:");
        p.increaseIndent();
        try {
            long match = map(CONFIGURATION_MAP_PATH, S32.class, U32.class)
                    .getValue(UID_RULES_CONFIGURATION_KEY).val;
            p.println("current ownerMatch configuration: " + match + " " + matchToString(match));
        } catch (ErrnoException e) { p.println("Failed to read ownerMatch configuration: " + e); }
        try {
            long config = map(CONFIGURATION_MAP_PATH, S32.class, U32.class)
                    .getValue(CURRENT_STATS_MAP_CONFIGURATION_KEY).val;
            p.println("current statsMap configuration: " + config + " "
                    + (config == 0 ? "SELECT_MAP_A" : "SELECT_MAP_B"));
        } catch (ErrnoException e) { p.println("Falied to read current statsMap configuration: " + e); }
        p.println();
        dumpMap(p, "sCookieTagMap", COOKIE_TAG_MAP_PATH, CookieTagMapKey.class, CookieTagMapValue.class,
                (k,v) -> "cookie=" + k.socketCookie + " tag=0x" + Long.toHexString(v.tag) + " uid=" + v.uid);
        dumpMap(p, "sUidOwnerMap", UID_OWNER_MAP_PATH, S32.class, UidOwnerValue.class,
                (k,v) -> k.val + " " + matchToString(v.rule) + ((v.rule & IIF_MATCH) != 0 ? " " + v.iif : ""));
        dumpMap(p, "sUidPermissionMap", UID_PERMISSION_MAP_PATH, S32.class, U8.class,
                (k,v) -> k.val + " " + permission(v.val));
        dumpMap(p, "sIngressDiscardMap", INGRESS_DISCARD_MAP_PATH, IngressDiscardKey.class, IngressDiscardValue.class,
                (k,v) -> "[" + k.dstAddr + "]: " + v.iif1 + "(" + Os.if_indextoname(v.iif1) + "), " + v.iif2 + "(" + Os.if_indextoname(v.iif2) + ")");
        dumpMap(p, "sLocalNetAccessMap (default is true meaning global)", LOCAL_NET_ACCESS_MAP_PATH,
                LocalNetAccessKey.class, Bool.class, (k,v) -> "" + k + ": " + v.val);
        dumpMap(p, "sLocalNetBlockedUidMap", LOCAL_NET_BLOCKED_UID_MAP_PATH, U32.class, Bool.class,
                (k,v) -> "" + k + ": " + v.val);
        try {
            p.println("sDataSaverEnabledMap: " + (map(DATA_SAVER_ENABLED_MAP_PATH, S32.class, U8.class)
                    .getValue(DATA_SAVER_ENABLED_KEY).val != DATA_SAVER_DISABLED));
        } catch (ErrnoException e) { p.println("Failed to read data saver configuration: " + e); }
        p.decreaseIndent();
    }
    private static String permission(int mask) {
        if (mask == 0) return "PERMISSION_NONE";
        if (mask == -1) return "PERMISSION_UNINSTALLED error!";
        var s = new java.util.StringJoiner(" ");
        if ((mask & 4) != 0) s.add("PERMISSION_INTERNET");
        if ((mask & 8) != 0) s.add("PERMISSION_UPDATE_DEVICE_STATS");
        if ((mask & ~12) != 0) s.add("PERMISSION_UNKNOWN(" + (mask & ~12) + ")");
        return s.toString();
    }
}
