package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.os.HandlerThread;
import android.os.IBinder;
import android.view.InputChannel;
import android.view.View;
import android.view.WindowManager;

/**
 * Independent controller/driver regression fixture.  Android Handler and
 * InputChannel are deterministic seams here; no native or Binder runtime is
 * linked, while the WMS publication classes under test are real sources.
 */
public final class WindowPublicationDriverTest {
    private static final int ACCEPTED = 0;
    private static final int BACKPRESSURED = 1;
    private static final int TERMINAL = 2;

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static WindowSessionIdentity identity(int pid, Object attachment) {
        return new WindowSessionIdentity(pid, pid + 1000, attachment);
    }

    private static WindowSessionWindowOwnership.Registration registration(IBinder window) {
        return WindowSessionWindowOwnership.registration(window);
    }

    private static WindowManager.LayoutParams params(int type, int flags, IBinder token) {
        WindowManager.LayoutParams value = new WindowManager.LayoutParams();
        value.type = type;
        value.flags = flags;
        value.token = token;
        return value;
    }

    private static void pump(int turns) {
        for (int i = 0; i < turns; ++i) {
            if (!HandlerThread.runLatestNext()) return;
        }
    }

    private static void testRegisterRelayoutRemoveAndUnboundFocus() {
        WindowInputPublisher.reset();
        WindowPublicationController controller = new WindowPublicationController();
        Object attachment = new Object();
        WindowSessionIdentity identity = identity(11, attachment);
        TestBinder token = new TestBinder("main");
        InputChannel channel = new InputChannel("main-channel");
        WindowSessionWindowOwnership.Registration registration = registration(token);

        controller.register(registration, identity, channel, params(1, 0, null), 0);
        check(controller.ownsOriginal(channel), "registered original channel is leased");
        controller.ready(registration);
        pump(4);
        check(WindowInputPublisher.eventCount(channel,
                WindowFocusRegistry.PublicationKind.GEOMETRY) == 1,
                "register publishes initial unbound geometry");
        check(WindowInputPublisher.eventCount(channel,
                WindowFocusRegistry.PublicationKind.FOCUS) == 0,
                "unbound controller window never receives focus");

        controller.relayout(registration, new Rect(0, 0, 500, 700), View.VISIBLE, null);
        pump(4);
        check(WindowInputPublisher.eventCount(channel,
                WindowFocusRegistry.PublicationKind.GEOMETRY) == 2,
                "relayout publishes real geometry");
        controller.remove(registration);
        // No Binder callback follows remove; the driver's delayed turn owns
        // final TX drain, terminal settlement, and channel release.
        pump(8);
        check(WindowInputPublisher.released(channel) && channel.disposed,
                "last remove progresses to release without future Binder");
    }

    private static void testAcceptedPendingBytesWaitForRetirement() {
        WindowInputPublisher.reset();
        WindowPublicationController controller = new WindowPublicationController();
        WindowSessionWindowOwnership.Registration registration =
                registration(new TestBinder("pending"));
        InputChannel channel = new InputChannel("pending-channel");
        WindowSessionIdentity identity = identity(12, new Object());
        controller.register(registration, identity, channel, params(1, 0, null), 0);
        controller.ready(registration);
        pump(1); // Accepted publication remains an outstanding TX prefix.
        WindowInputPublisher.holdFlush(channel, true);
        controller.remove(registration);
        pump(1);
        check(!WindowInputPublisher.released(channel),
                "accepted pending bytes prevent premature retirement release");
        WindowInputPublisher.holdFlush(channel, false);
        pump(8);
        check(WindowInputPublisher.released(channel),
                "retirement releases after accepted TX flushes");
    }

    private static void testBackpressureThenAccept() {
        WindowInputPublisher.reset();
        WindowPublicationController controller = new WindowPublicationController();
        WindowSessionWindowOwnership.Registration registration =
                registration(new TestBinder("backpressure"));
        InputChannel channel = new InputChannel("backpressure-channel");
        WindowInputPublisher.scriptPublish(channel, BACKPRESSURED, ACCEPTED);
        controller.register(registration, identity(13, new Object()), channel,
                params(1, 0, null), 0);
        controller.ready(registration);
        pump(1);
        check(WindowInputPublisher.eventCount(channel) == 0,
                "backpressure does not fabricate publication acceptance");
        pump(8);
        check(WindowInputPublisher.eventCount(channel) == 1,
                "backpressured publication retries and accepts");
        controller.remove(registration);
        pump(8);
        check(WindowInputPublisher.released(channel), "backpressure endpoint retires");
    }

    private static void testTerminalFalseThenTrue() {
        WindowInputPublisher.reset();
        WindowPublicationController controller = new WindowPublicationController();
        WindowSessionWindowOwnership.Registration registration =
                registration(new TestBinder("terminal"));
        InputChannel channel = new InputChannel("terminal-channel");
        WindowInputPublisher.scriptPublish(channel, TERMINAL);
        WindowInputPublisher.scriptTermination(channel, false, true);
        controller.register(registration, identity(14, new Object()), channel,
                params(1, 0, null), 0);
        controller.ready(registration);
        pump(1);
        controller.remove(registration);
        pump(1);
        check(!WindowInputPublisher.released(channel),
                "false terminal settlement preserves lease and queue");
        pump(8);
        check(WindowInputPublisher.released(channel),
                "true terminal settlement eventually retires exact endpoint");
        check(WindowInputPublisher.eventCount(channel) == 1,
                "terminal publication is not replayed after settlement");
    }

    private static void testStaleReplacementIndependence() {
        WindowInputPublisher.reset();
        WindowPublicationController controller = new WindowPublicationController();
        TestBinder token = new TestBinder("reused-token");
        WindowSessionIdentity identity = identity(15, new Object());
        InputChannel oldChannel = new InputChannel("old-channel");
        WindowSessionWindowOwnership.Registration oldRegistration = registration(token);
        controller.register(oldRegistration, identity, oldChannel, params(1, 0, null), 0);
        controller.ready(oldRegistration);
        pump(8);
        controller.remove(oldRegistration);

        InputChannel newChannel = new InputChannel("new-channel");
        WindowSessionWindowOwnership.Registration newRegistration = registration(token);
        controller.register(newRegistration, identity, newChannel, params(1, 0, null), 0);
        controller.ready(newRegistration);
        pump(10);
        check(WindowInputPublisher.eventCount(oldChannel) == 2,
                "stale remove publication stays on original channel");
        check(WindowInputPublisher.eventCount(newChannel) == 1,
                "successor publication is isolated from stale channel");
        check(WindowInputPublisher.released(oldChannel)
                && !WindowInputPublisher.released(newChannel),
                "stale endpoint retires independently of successor");
        controller.remove(newRegistration);
        pump(8);
        check(WindowInputPublisher.released(newChannel), "successor retires normally");
    }

    private static void testGenerationSchedulesNewEndpointDuringFlush() {
        WindowInputPublisher.reset();
        WindowPublicationController controller = new WindowPublicationController();
        WindowSessionIdentity identity = identity(16, new Object());
        InputChannel oldChannel = new InputChannel("generation-old");
        InputChannel newChannel = new InputChannel("generation-new");
        WindowSessionWindowOwnership.Registration oldRegistration =
                registration(new TestBinder("generation-old-token"));
        WindowSessionWindowOwnership.Registration newRegistration =
                registration(new TestBinder("generation-new-token"));
        WindowInputPublisher.immediateAck(oldChannel, true);
        WindowInputPublisher.onNextFlush(oldChannel, () -> {
            controller.register(newRegistration, identity, newChannel, params(1, 0, null), 0);
            controller.ready(newRegistration);
        });
        controller.register(oldRegistration, identity, oldChannel, params(1, 0, null), 0);
        controller.ready(oldRegistration);
        check(HandlerThread.runLatestNext(), "initial old endpoint turn ran");
        check(WindowInputPublisher.eventCount(newChannel) == 1,
                "new endpoint accepted publication during old flush turn");
        check(WindowInputPublisher.flushCount(newChannel) == 0,
                "new endpoint was absent from old turn snapshot");
        check(HandlerThread.runLatestNext(),
                "work generation queued a follow-up turn without Binder");
        check(WindowInputPublisher.flushCount(newChannel) == 1,
                "follow-up turn flushed new endpoint");
        controller.remove(oldRegistration);
        controller.remove(newRegistration);
        pump(12);
    }

    private static void testParentAttachmentAndDisplayValidation() {
        WindowInputPublisher.reset();
        WindowPublicationController controller = new WindowPublicationController();
        Object attachment = new Object();
        WindowSessionIdentity owner = identity(17, attachment);
        TestBinder parentToken = new TestBinder("parent");
        WindowSessionWindowOwnership.Registration parent = registration(parentToken);
        InputChannel parentChannel = new InputChannel("parent-channel");
        controller.register(parent, owner, parentChannel, params(1, 0, null), 0);
        controller.ready(parent);
        pump(4);

        WindowSessionWindowOwnership.Registration child = registration(
                new TestBinder("child"));
        InputChannel childChannel = new InputChannel("child-channel");
        controller.register(child, owner, childChannel, params(1000, 0, parentToken), 0);
        controller.ready(child);
        pump(4);

        boolean wrongParent = false;
        try {
            controller.register(registration(new TestBinder("wrong-child")), owner,
                    new InputChannel("wrong-child-channel"),
                    params(1000, 0, new TestBinder("unknown-parent")), 0);
        } catch (SecurityException expected) { wrongParent = true; }
        check(wrongParent, "attached child requires exact canonical parent");

        boolean foreignAttachment = false;
        try {
            controller.register(registration(new TestBinder("foreign-child")),
                    identity(17, new Object()), new InputChannel("foreign-child-channel"),
                    params(1000, 0, parentToken), 0);
        } catch (SecurityException expected) { foreignAttachment = true; }
        check(foreignAttachment, "attached child requires complete same attachment");

        boolean wrongDisplay = false;
        try {
            controller.register(registration(new TestBinder("display")), owner,
                    new InputChannel("display-channel"), params(1, 0, null), 1);
        } catch (IllegalArgumentException expected) { wrongDisplay = true; }
        check(wrongDisplay, "unknown display is rejected before leasing");

        controller.remove(child);
        controller.remove(parent);
        pump(12);
    }

    private static void testSchedulerPostFailureLeavesLease() {
        WindowInputPublisher.reset();
        WindowPublicationController controller = new WindowPublicationController();
        HandlerThread.latest().setFailPosts(true);
        InputChannel channel = new InputChannel("scheduler-failure-channel");
        boolean failed = false;
        try {
            controller.register(registration(new TestBinder("scheduler-failure")),
                    identity(18, new Object()), channel, params(1, 0, null), 0);
        } catch (IllegalStateException expected) { failed = true; }
        check(failed, "scheduler post failure is surfaced");
        check(controller.ownsOriginal(channel) && !channel.disposed,
                "scheduler post failure leaves lease for recovery, not disposal");

        WindowInputPublisher.reset();
        WindowPublicationController throwingController = new WindowPublicationController();
        HandlerThread.latest().setThrowPosts(true);
        InputChannel throwingChannel = new InputChannel("scheduler-throw-channel");
        boolean threw = false;
        try {
            throwingController.register(registration(new TestBinder("scheduler-throw")),
                    identity(19, new Object()), throwingChannel, params(1, 0, null), 0);
        } catch (AssertionError expected) { threw = true; }
        check(threw && throwingController.ownsOriginal(throwingChannel)
                && !throwingChannel.disposed,
                "scheduler post Error leaves exact lease for recovery");
    }

    public static void main(String[] args) {
        testRegisterRelayoutRemoveAndUnboundFocus();
        testAcceptedPendingBytesWaitForRetirement();
        testBackpressureThenAccept();
        testTerminalFalseThenTrue();
        testStaleReplacementIndependence();
        testGenerationSchedulesNewEndpointDuringFlush();
        testParentAttachmentAndDisplayValidation();
        testSchedulerPostFailureLeavesLease();
        System.out.println("WMS publication controller/driver deterministic fixture: PASS");
    }
}
