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
    // Held by APP_UID (and the system) in these tests' permission state.
    private static final String SEND_PERMISSION = "example.permission.SEND";

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
                delivered.add(new Delivered(receiver, intent, sticky, uid)),
                (uid, permission) -> uid == SYSTEM_UID
                        || (uid == APP_UID && SEND_PERMISSION.equals(permission)));
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
        permissionsDecideReachability();
        allowListAndExtrasFilterNarrowDelivery();
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
        registry.broadcast(OTHER_UID, "other", new Intent(OTHER_ACTION), null, null, null, null, false, 0);
        check(delivered.isEmpty(), "non-matching action is not delivered");
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, null, null, null, false, 0);
        check(delivered.size() == 1 && delivered.get(0).receiver == receiver
                && !delivered.get(0).sticky && delivered.get(0).sendingUid == OTHER_UID,
                "matching broadcast reaches the exported receiver");
    }

    private static void nonExportedReceiversOnlyHearOwnUidAndSystem() {
        BroadcastRegistry registry = registry();
        registry.register(app(APP_UID, "app"), new Binder(), new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_NOT_EXPORTED);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, null, null, null, false, 0);
        check(delivered.isEmpty(), "another app cannot reach a non-exported receiver");
        registry.broadcast(APP_UID, "app", new Intent(ACTION), null, null, null, null, false, 0);
        registry.broadcast(SYSTEM_UID, "android", new Intent(ACTION), null, null, null, null, false,
                BroadcastRegistry.USER_ALL);
        check(delivered.size() == 2, "own uid and system reach a non-exported receiver");
    }

    private static void permissionsDecideReachability() {
        BroadcastRegistry registry = registry();
        registry.register(app(APP_UID, "app"), new Binder(), new IntentFilter(ACTION),
                SEND_PERMISSION, 0, BroadcastRegistry.RECEIVER_EXPORTED);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, null, null, null, false,
                0);
        check(delivered.isEmpty(), "a sender without the receiver's permission is skipped");
        registry.broadcast(APP_UID, "app", new Intent(ACTION), null, null, null, null, false, 0);
        check(delivered.size() == 1, "a sender holding the permission reaches the receiver");
        BroadcastRegistry open = registry();
        open.register(app(OTHER_UID, "other"), new Binder(), new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED);
        open.broadcast(SYSTEM_UID, "android", new Intent(ACTION), null,
                new String[] {SEND_PERMISSION}, null, null, false, 0);
        check(delivered.isEmpty(), "a receiver without the broadcast's permission is skipped");
    }

    private static void allowListAndExtrasFilterNarrowDelivery() {
        BroadcastRegistry registry = registry();
        registry.register(app(APP_UID, "app"), new Binder(), new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED);
        registry.register(app(OTHER_UID, "other"), new Binder(), new IntentFilter(ACTION), null,
                0, BroadcastRegistry.RECEIVER_EXPORTED);
        registry.broadcast(SYSTEM_UID, "android", new Intent(ACTION), null, null,
                new int[] {APP_UID}, null, false, 0);
        check(delivered.size() == 1 && delivered.get(0).intent.getPackage() == null,
                "only allow-listed app ids receive");
        delivered.clear();
        registry.broadcast(SYSTEM_UID, "android", new Intent(ACTION).putExtra("secret", 1), null,
                null, null, (uid, sent) -> uid == APP_UID ? sent : null, false, 0);
        check(delivered.size() == 1 && delivered.get(0).intent.getIntExtra("secret", 0) == 1,
                "a receiver's intent can withhold the broadcast from another uid");
    }

    private static void packageTargetedBroadcastsSkipOtherPackages() {
        BroadcastRegistry registry = registry();
        registry.register(app(APP_UID, "app"), new Binder(), new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION).setPackage("elsewhere"), null,
                null, null, null, false, 0);
        check(delivered.isEmpty(), "a broadcast for another package is not delivered");
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION).setPackage("app"), null,
                null, null, null, false, 0);
        check(delivered.size() == 1, "a broadcast for the receiver's package is delivered");
    }

    private static void stickyIsReturnedAndRedelivered() {
        BroadcastRegistry registry = registry();
        Intent battery = new Intent(Intent.ACTION_BATTERY_CHANGED).putExtra("level", 80);
        registry.broadcast(SYSTEM_UID, "android", battery, null, null, null, null, true,
                BroadcastRegistry.USER_ALL);
        Intent query = registry.register(app(APP_UID, "app"), null,
                new IntentFilter(Intent.ACTION_BATTERY_CHANGED), null, 0, 0);
        check(query != null && query.getIntExtra("level", -1) == 80,
                "a null receiver reads the current sticky");
        registry.broadcast(SYSTEM_UID, "android",
                new Intent(Intent.ACTION_BATTERY_CHANGED).putExtra("level", 79), null, null, null, null,
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
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, null, null, null, false, 0);
        check(delivered.isEmpty(), "an unregistered receiver is not delivered");
        registry.register(owner, new Binder(), new IntentFilter(ACTION), null, 0,
                BroadcastRegistry.RECEIVER_EXPORTED);
        registry.processGone(owner.pid);
        registry.broadcast(OTHER_UID, "other", new Intent(ACTION), null, null, null, null, false, 0);
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
