package dev.darwinart.runtime.am;

import android.content.Intent;
import android.content.IntentFilter;
import android.os.Binder;
import android.os.IBinder;
import java.util.ArrayList;
import java.util.List;

/**
 * Registered-receiver broadcasts: filter matching, export and permission
 * reachability, package targeting, sticky return and redelivery, and removal.
 */
public final class BroadcastRegistryTest {
    private static final int APP_UID = 10042;
    private static final int OTHER_UID = 10043;
    private static final int SYSTEM_UID = 1000;
    private static final String ACTION = "example.ACTION";
    private static final String OTHER_ACTION = "example.OTHER";

    private static final class Delivered {
        final IBinder receiver;
        final Intent intent;
        final boolean sticky;
        final int sendingUid;

        Delivered(IBinder receiver, Intent intent, boolean sticky, int sendingUid) {
            this.receiver = receiver;
            this.intent = intent;
            this.sticky = sticky;
            this.sendingUid = sendingUid;
        }
    }

    private static final List<Delivered> delivered = new ArrayList<>();
    private static int nextPid = 100;

    private static BroadcastRegistry registry() {
        delivered.clear();
        return new BroadcastRegistry((thread, receiver, intent, sticky, user, uid, pkg) ->
                delivered.add(new Delivered(receiver, intent, sticky, uid)));
    }

    private static ApplicationProcessRegistry.AttachedApplication app(int uid, String pkg) {
        ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        int pid = nextPid++;
        Binder thread = new Binder();
        processes.beginAttachment(pid, uid, thread, 1);
        processes.identify(pid, 1, thread, pkg);
        return processes.finishAttachment(pid, 1);
    }

    public static void main(String[] args) {
        deliversToMatchingReceivers();
        nonExportedReceiversOnlyHearOwnUidAndSystem();
        permissionGuardedReceiversOnlyHearOwnUid();
        packageTargetedBroadcastsSkipOtherPackages();
        stickyIsReturnedAndRedelivered();
        unregisterAndProcessDeathStopDelivery();
        conflictingExportFlagsAreRejected();
        System.out.println("BroadcastRegistryTest PASS");
    }

    private static void deliversToMatchingReceivers() {
        BroadcastRegistry registry = registry();
        Binder receiver = new Binder();
        check(registry.register(app(APP_UID, "app"), receiver, new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED) == null, "no sticky yet");
        registry.broadcast(OTHER_UID, "other", new Intent(OTHER_ACTION), null, false, false, 0);
        check(delivered.isEmpty(), "non-matching action is not delivered");
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, false, false, 0);
        check(delivered.size() == 1 && delivered.get(0).receiver == receiver
                && !delivered.get(0).sticky && delivered.get(0).sendingUid == OTHER_UID,
                "matching broadcast reaches the exported receiver");
    }

    private static void nonExportedReceiversOnlyHearOwnUidAndSystem() {
        BroadcastRegistry registry = registry();
        registry.register(app(APP_UID, "app"), new Binder(), new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_NOT_EXPORTED);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, false, false, 0);
        check(delivered.isEmpty(), "another app cannot reach a non-exported receiver");
        registry.broadcast(APP_UID, "app", new Intent(ACTION), null, false, false, 0);
        registry.broadcast(SYSTEM_UID, "android", new Intent(ACTION), null, false, false,
                BroadcastRegistry.USER_ALL);
        check(delivered.size() == 2, "own uid and system reach a non-exported receiver");
    }

    private static void permissionGuardedReceiversOnlyHearOwnUid() {
        BroadcastRegistry registry = registry();
        registry.register(app(APP_UID, "app"), new Binder(), new IntentFilter(ACTION),
                "example.permission.SEND", 0, BroadcastRegistry.RECEIVER_EXPORTED);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, false, false, 0);
        check(delivered.isEmpty(), "unmodeled permission grants do not reach other uids");
        registry.broadcast(APP_UID, "app", new Intent(ACTION), null, false, false, 0);
        check(delivered.size() == 1, "own uid reaches its permission-guarded receiver");
    }

    private static void packageTargetedBroadcastsSkipOtherPackages() {
        BroadcastRegistry registry = registry();
        registry.register(app(APP_UID, "app"), new Binder(), new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION).setPackage("elsewhere"), null,
                false, false, 0);
        check(delivered.isEmpty(), "a broadcast for another package is not delivered");
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION).setPackage("app"), null,
                false, false, 0);
        check(delivered.size() == 1, "a broadcast for the receiver's package is delivered");
    }

    private static void stickyIsReturnedAndRedelivered() {
        BroadcastRegistry registry = registry();
        Intent battery = new Intent(Intent.ACTION_BATTERY_CHANGED).putExtra("level", 80);
        registry.broadcast(SYSTEM_UID, "android", battery, null, false, true,
                BroadcastRegistry.USER_ALL);
        Intent query = registry.register(app(APP_UID, "app"), null,
                new IntentFilter(Intent.ACTION_BATTERY_CHANGED), null, 0, 0);
        check(query != null && query.getIntExtra("level", -1) == 80,
                "a null receiver reads the current sticky");
        registry.broadcast(SYSTEM_UID, "android",
                new Intent(Intent.ACTION_BATTERY_CHANGED).putExtra("level", 79), null, false,
                true, BroadcastRegistry.USER_ALL);
        Binder receiver = new Binder();
        delivered.clear();
        Intent sticky = registry.register(app(APP_UID, "app"), receiver,
                new IntentFilter(Intent.ACTION_BATTERY_CHANGED), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED);
        check(sticky != null && sticky.getIntExtra("level", -1) == 79,
                "a newer sticky replaces the filter-equal one");
        check(delivered.size() == 1 && delivered.get(0).sticky
                && delivered.get(0).sendingUid == SYSTEM_UID,
                "the sticky is redelivered to a newly registered receiver");
    }

    private static void unregisterAndProcessDeathStopDelivery() {
        BroadcastRegistry registry = registry();
        Binder first = new Binder();
        ApplicationProcessRegistry.AttachedApplication owner = app(APP_UID, "app");
        registry.register(owner, first, new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED);
        registry.unregister(first);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, false, false, 0);
        check(delivered.isEmpty(), "an unregistered receiver is not delivered");
        registry.register(owner, new Binder(), new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED);
        registry.processGone(owner.pid);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, false, false, 0);
        check(delivered.isEmpty(), "process death removes its registrations");
    }

    private static void conflictingExportFlagsAreRejected() {
        boolean rejected = false;
        try {
            registry().register(app(APP_UID, "app"), new Binder(), new IntentFilter(ACTION),
                    null, 0, BroadcastRegistry.RECEIVER_EXPORTED
                            | BroadcastRegistry.RECEIVER_NOT_EXPORTED);
        } catch (IllegalArgumentException expected) {
            rejected = true;
        }
        check(rejected, "both export flags are rejected");
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
}
