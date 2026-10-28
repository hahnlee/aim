package dev.aim.server;
import android.content.Context;
import android.net.*;
import android.net.ConnectivityDiagnosticsManager.*;
import android.os.*;
import java.util.ArrayList;

/** ConnectivityDiagnosticsManagerTest's cached delivery, permission, matching and lifetime cases. */
public final class ConnectivityDiagnosticsTest {
    static final class Callback extends IConnectivityDiagnosticsCallback.Stub {
        final ArrayList<Object> events = new ArrayList<>();
        @Override public void onConnectivityReportAvailable(ConnectivityReport report) { events.add(report); }
        @Override public void onNetworkConnectivityReported(Network network, boolean connected) { events.add(connected); }
        @Override public void onDataStallSuspected(DataStallReport report) { throw new AssertionError("no detector"); }
    }
    static void check(boolean value) { if (!value) throw new AssertionError(); }
    public static void main(String[] args) {
        Context context = new Context();
        Handler handler = new Handler();
        ArrayList<Integer> releases = new ArrayList<>();
        NetworkActivityBridge bridge = new NetworkActivityBridge(context, handler, releases::add);
        NetworkCapabilities ethernet = new NetworkCapabilities();
        Network network = new Network(100);
        bridge.onDiagnosticsNetworkChanged(network, new LinkProperties(), ethernet, 123);
        Callback app = new Callback(), stack = new Callback(), mainline = new Callback(), sysui = new Callback(), wifi = new Callback();
        bridge.registerDiagnostics(app, ethernet, 10100, 20, 1);
        bridge.registerDiagnostics(app, ethernet, 10100, 20, 99);
        context.grants.add("10101:android.permission.NETWORK_STACK");
        context.grants.add("10102:android.permission.MAINLINE_NETWORK_STACK");
        context.grants.add("10103:android.permission.STATUS_BAR_SERVICE");
        bridge.registerDiagnostics(stack, ethernet, 10101, 21, 2);
        bridge.registerDiagnostics(mainline, ethernet, 10102, 22, 3);
        bridge.registerDiagnostics(sysui, ethernet, 10103, 23, 4);
        NetworkCapabilities wifiRequest = new NetworkCapabilities(); wifiRequest.transport = 1;
        bridge.registerDiagnostics(wifi, wifiRequest, 10101, 21, 5);
        handler.drain();
        check(app.events.isEmpty() && wifi.events.isEmpty());
        check(stack.events.size() == 1 && mainline.events.size() == 1 && sysui.events.size() == 1);
        ConnectivityReport report = (ConnectivityReport) stack.events.get(0);
        check(report.timestamp == 123 && report.network.equals(network));
        check(report.caps.owner == -1 && report.caps.administrators.length == 0 && report.caps.uids == null);
        check(report.extras.values.get(ConnectivityReport.KEY_NETWORK_VALIDATION_RESULT) == 3);
        check(report.extras.values.get(ConnectivityReport.KEY_NETWORK_PROBES_ATTEMPTED_BITMASK) == 0);
        bridge.onNetworkConnectivityReported(network, 10101, true, false);
        handler.drain();
        check(stack.events.size() == 3 && stack.events.get(1).equals(true) && stack.events.get(2) == report);
        check(mainline.events.size() == 1);
        bridge.onNetworkConnectivityReported(network, 10101, false, true);
        bridge.onDiagnosticsNetworkChanged(network, new LinkProperties(), ethernet, 456);
        handler.drain();
        check(mainline.events.size() == 3 && mainline.events.get(1).equals(false));
        check(((ConnectivityReport)mainline.events.get(2)).timestamp == 456);
        context.grants.clear();
        bridge.onNetworkConnectivityReported(network, 10101, true, false);
        handler.drain();
        check(stack.events.size() == 5); // permissions are checked again for every delivery
        bridge.unregisterDiagnostics(stack, 10100);
        handler.drain(); check(releases.equals(java.util.List.of(99)));
        bridge.unregisterDiagnostics(stack, 10101);
        bridge.unregisterDiagnostics(stack, 10101);
        handler.drain(); check(releases.equals(java.util.List.of(99, 2)));
        app.die();
        handler.drain(); check(releases.equals(java.util.List.of(99, 2, 1)));
        bridge.onDiagnosticsNetworkChanged(null, null, null, 0);
        handler.drain();
        context.grants.add("10101:android.permission.NETWORK_STACK");
        Callback offline = new Callback();
        bridge.registerDiagnostics(offline, ethernet, 10101, 21, 6);
        handler.drain(); check(offline.events.isEmpty());
        bridge.onDiagnosticsNetworkChanged(new Network(101), new LinkProperties(), ethernet, 789);
        handler.drain(); check(offline.events.size() == 1);
        Callback dead = new Callback(); dead.die();
        bridge.registerDiagnostics(dead, ethernet, 10101, 21, 7);
        handler.drain(); check(releases.contains(7) && dead.events.isEmpty());
        Binder.uid = 10100;
        try { bridge.registerDiagnostics(offline, ethernet, 10100, 20, 9); throw new AssertionError(); }
        catch (SecurityException expected) { }
        try { bridge.unregisterDiagnostics(offline, 10100); throw new AssertionError(); }
        catch (SecurityException expected) { }
        try { bridge.onDiagnosticsNetworkChanged(null, null, null, 0); throw new AssertionError(); }
        catch (SecurityException expected) { }
        try { bridge.onNetworkConnectivityReported(network, 10100, true, false); throw new AssertionError(); }
        catch (SecurityException expected) { }
        Binder.uid = 1000;
        android.provider.DeviceConfig.flag = "-1";
        NetworkActivityBridge disabled = new NetworkActivityBridge(context, handler, releases::add);
        context.grants.add("10103:android.permission.STATUS_BAR_SERVICE");
        Callback deniedSysui = new Callback();
        disabled.registerDiagnostics(deniedSysui, ethernet, 10103, 23, 10);
        disabled.onDiagnosticsNetworkChanged(network, new LinkProperties(), ethernet, 900);
        handler.drain(); check(deniedSysui.events.isEmpty());
    }
}
