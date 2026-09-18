package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.view.IWindowManager;
import android.view.IWindowSession;
import android.view.InputChannel;
import android.view.View;
import android.view.WindowManager;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Exercises host-root facts through the real WMS root endpoint and publication
 * controller.  The foreground provider is the only host-fact test seam; all
 * window selection and original-channel publication remain production code.
 */
public final class DesktopRootActivationTest {
    private static final int PID_A = 4101;
    private static final int UID_A = 10101;
    private static final int PID_B = 4102;
    private static final int UID_B = 10102;
    private static int checks;

    private static final class Foreground implements DesktopForegroundAuthority.Provider {
        int foregroundPid;
        int captures;
        DesktopForegroundAuthority.ProcessInstance lastCapture;
        DesktopForegroundAuthority.ProcessInstance lastQuery;
        Runnable onQuery;

        Foreground(int pid) { foregroundPid = pid; }

        @Override public DesktopForegroundAuthority.ProcessInstance capture(int pid) {
            ++captures;
            lastCapture = new DesktopForegroundAuthority.ProcessInstance(pid, 1, 7);
            return lastCapture;
        }

        @Override public boolean isForeground(DesktopForegroundAuthority.ProcessInstance process) {
            lastQuery = process;
            Runnable callback = onQuery;
            onQuery = null;
            if (callback != null) callback.run();
            return process != null && process.pid == foregroundPid
                    && process.startSeconds == 1 && process.startMicroseconds == 7;
        }
    }

    private static void check(boolean value, String message) {
        ++checks;
        if (!value) throw new AssertionError(message);
    }

    private static Object field(Object owner, String name) throws Exception {
        Field value = owner.getClass().getDeclaredField(name);
        value.setAccessible(true);
        return value.get(owner);
    }

    private static final class Fixture {
        final Foreground foreground;
        final ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        final DesktopWindowMetadataRegistry metadata = new DesktopWindowMetadataRegistry();
        final WindowManagerEndpoint manager = new WindowManagerEndpoint(processes, metadata);
        final DesktopRootEndpoint roots;

        Fixture(int initialForegroundPid) {
            foreground = new Foreground(initialForegroundPid);
            roots = new DesktopRootEndpoint(processes,
                    manager.createDesktopRootRegistry(foreground));
        }

        WindowSessionEndpoint session(int pid, int uid, IBinder thread) throws Exception {
            processes.beginAttachment(pid, uid, thread, 1);
            processes.identify(pid, 1, thread, "fixture.activation." + pid);
            return openSession(pid, uid, thread);
        }

        WindowSessionEndpoint openSession(int pid, int uid, IBinder thread) throws Exception {
            Binder.setCallingIdentity(pid, uid);
            Parcel data = Parcel.obtain();
            data.writeInterfaceToken("android.view.IWindowManager");
            data.writeStrongBinder(thread);
            Parcel reply = Parcel.obtain();
            check(manager.transact(IWindowManager.Stub.TRANSACTION_openSession,
                    data, reply, 0), "openSession handled");
            reply.readException();
            return (WindowSessionEndpoint) reply.readStrongBinder();
        }

        void attach(int pid, int uid, IBinder thread) {
            processes.beginAttachment(pid, uid, thread, 1);
            processes.identify(pid, 1, thread, "fixture.activation." + pid);
        }
    }

    private static WindowManager.LayoutParams attrs() {
        WindowManager.LayoutParams attrs = new WindowManager.LayoutParams();
        attrs.type = WindowManager.LayoutParams.FIRST_APPLICATION_WINDOW;
        attrs.width = 320;
        attrs.height = 240;
        return attrs;
    }

    private static void add(WindowSessionEndpoint session, IBinder window) throws Exception {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken("android.view.IWindowSession");
        data.writeStrongBinder(window);
        data.writeTypedObject(attrs(), 0);
        data.writeInt(View.VISIBLE);
        data.writeInt(0); // display layer stack
        data.writeInt(0); // user id
        data.writeInt(0); // requested insets
        data.writeInt(1); // scale array length
        Parcel reply = Parcel.obtain();
        check(session.transact(IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                data, reply, 0), "window add handled");
        reply.readException();
        data = Parcel.obtain();
        data.writeInterfaceToken("android.view.IWindowSession");
        data.writeStrongBinder(window);
        data.writeTypedObject(attrs(), 0);
        data.writeInt(320); // requested width
        data.writeInt(240); // requested height
        data.writeInt(View.VISIBLE);
        data.writeInt(0); // relayout flags
        data.writeInt(1); // sequence
        data.writeInt(0); // last sync sequence
        reply = Parcel.obtain();
        check(session.transact(IWindowSession.Stub.TRANSACTION_relayout,
                data, reply, 0), "window relayout handled");
        reply.readException();
        pump();
    }

    private static void relayout(WindowSessionEndpoint session, IBinder window) throws Exception {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken("android.view.IWindowSession");
        data.writeStrongBinder(window);
        data.writeTypedObject(attrs(), 0);
        data.writeInt(320);
        data.writeInt(240);
        data.writeInt(View.VISIBLE);
        data.writeInt(0);
        data.writeInt(2);
        data.writeInt(0);
        Parcel reply = Parcel.obtain();
        check(session.transact(IWindowSession.Stub.TRANSACTION_relayout,
                data, reply, 0), "window relayout handled");
        reply.readException();
    }

    private static void remove(WindowSessionEndpoint session, IBinder window,
            int pid, int uid) throws Exception {
        Binder.setCallingIdentity(pid, uid);
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken("android.view.IWindowSession");
        data.writeStrongBinder(window);
        Parcel reply = Parcel.obtain();
        check(session.transact(IWindowSession.Stub.TRANSACTION_remove, data, reply, 0),
                "window remove handled");
        reply.readException();
    }

    @SuppressWarnings("unchecked")
    private static InputChannel channel(WindowSessionEndpoint session, IBinder window)
            throws Exception {
        Map<WindowSessionWindowOwnership.Registration, InputChannel> channels =
                (Map<WindowSessionWindowOwnership.Registration, InputChannel>)
                        field(session, "serverInputChannels");
        for (Map.Entry<WindowSessionWindowOwnership.Registration, InputChannel> entry
                : channels.entrySet()) {
            if (entry.getKey().window().equals(window)) return entry.getValue();
        }
        throw new AssertionError("missing original server channel");
    }

    private static WindowFocusRegistry registry(Fixture fixture) throws Exception {
        Object publications = field(fixture.manager, "publications");
        return (WindowFocusRegistry) field(publications, "registry");
    }

    private static WindowPublicationDriver driver(Fixture fixture) throws Exception {
        Object publications = field(fixture.manager, "publications");
        return (WindowPublicationDriver) field(publications, "driver");
    }

    private static void pump() {
        for (int i = 0; i < 64 && android.os.HandlerThread.runLatestNext(); ++i) {}
    }

    private static IBinder register(Fixture fixture, int pid, int uid, long incarnation,
            Binder lifetime) throws Exception {
        Binder.setCallingIdentity(pid, uid);
        return DesktopRootProtocol.register(fixture.roots, incarnation, lifetime);
    }

    private static void bind(IBinder capability, InputChannel channel, int pid, int uid)
            throws Exception {
        Binder.setCallingIdentity(pid, uid);
        DesktopRootProtocol.bindWindow(capability, channel.getToken(), 0);
    }

    private static void fact(IBinder capability, int pid, int uid, int kind,
            long incarnation, long serial, boolean key) throws Exception {
        Binder.setCallingIdentity(pid, uid);
        DesktopRootProtocol.fact(capability, kind, incarnation, serial, key);
    }

    private static boolean pendingFocus(WindowFocusRegistry registry, InputChannel original,
            boolean focused) throws Exception {
        @SuppressWarnings("unchecked")
        List<WindowFocusRegistry.PublicationBatch> batches =
                (List<WindowFocusRegistry.PublicationBatch>) field(registry, "pending");
        for (WindowFocusRegistry.PublicationBatch batch : batches) {
            for (WindowFocusRegistry.Publication publication : batch.publications) {
                if (publication.kind == WindowFocusRegistry.PublicationKind.FOCUS
                        && publication.focused == focused
                        && publication.channelIncarnation instanceof WindowInputEndpoint
                        && ((WindowInputEndpoint) publication.channelIncarnation)
                                .owns(original)) return true;
            }
        }
        return false;
    }

    private static WindowFocusRegistry.WindowSpec spec(WindowFocusRegistry registry,
            IBinder window) throws Exception {
        @SuppressWarnings("unchecked")
        Map<Object, Object> windows = (Map<Object, Object>) field(registry, "windows");
        for (Object stored : windows.values()) {
            WindowFocusRegistry.WindowSpec value =
                    (WindowFocusRegistry.WindowSpec) field(stored, "spec");
            if (((WindowSessionWindowOwnership.Registration) value.windowToken)
                    .window().equals(window)) return value;
        }
        throw new AssertionError("missing canonical window spec");
    }

    private static void runUntilActive(WindowFocusRegistry registry) {
        for (int i = 0; i < 64 && registry.activeRoot(0) == null; ++i) {
            check(android.os.HandlerThread.runLatestNext(), "WMS handler work missing");
        }
    }

    private static void testProviderFalseAndBirthPinned() throws Exception {
        DesktopWindowMetadataRegistry.reset();
        WindowInputPublisher.reset();
        Fixture fixture = new Fixture(PID_B);
        Binder thread = new Binder();
        WindowSessionEndpoint session = fixture.session(PID_A, UID_A, thread);
        Binder window = new Binder();
        Binder.setCallingIdentity(PID_A, UID_A);
        add(session, window);
        InputChannel original = channel(session, window);
        Binder lifetime = new Binder();
        IBinder capability = register(fixture, PID_A, UID_A, 1, lifetime);
        check(fixture.foreground.captures == 1,
                "registration pins one immutable host process instance captures="
                        + fixture.foreground.captures);
        check(fixture.foreground.lastQuery == null,
                "provider must not query foreground during registration");
        bind(capability, original, PID_A, UID_A);
        fact(capability, PID_A, UID_A, DesktopRootRegistry.ACTIVATED, 1, 1, true);
        pump();
        check(fixture.foreground.lastQuery == fixture.foreground.lastCapture,
                "activation queries the pinned process instance");
        WindowFocusRegistry registry = registry(fixture);
        check(registry.activeRoot(0) == null && registry.focusedWindow(0) == null,
                "provider false fails closed without activation");
        check(!pendingFocus(registry, original, true),
                "provider false fabricated no focus publication");
    }

    private static void testActivationSelectionAndLateResign() throws Exception {
        DesktopWindowMetadataRegistry.reset();
        WindowInputPublisher.reset();
        Fixture fixture = new Fixture(PID_A);
        WindowSessionEndpoint sessionA = fixture.session(PID_A, UID_A, new Binder());
        WindowSessionEndpoint sessionB = fixture.session(PID_B, UID_B, new Binder());
        Binder windowA = new Binder(), windowB = new Binder();
        Binder.setCallingIdentity(PID_A, UID_A);
        add(sessionA, windowA);
        Binder.setCallingIdentity(PID_B, UID_B);
        add(sessionB, windowB);
        InputChannel channelA = channel(sessionA, windowA);
        InputChannel channelB = channel(sessionB, windowB);
        Binder rootLifetimeA = new Binder(), rootLifetimeB = new Binder();
        IBinder rootA = register(fixture, PID_A, UID_A, 11, rootLifetimeA);
        IBinder rootB = register(fixture, PID_B, UID_B, 12, rootLifetimeB);
        bind(rootA, channelA, PID_A, UID_A);
        bind(rootB, channelB, PID_B, UID_B);

        // A's event is rejected while B is the freshly observed host foreground.
        fixture.foreground.foregroundPid = PID_B;
        fact(rootA, PID_A, UID_A, DesktopRootRegistry.ACTIVATED, 11, 1, true);
        WindowFocusRegistry registry = registry(fixture);
        pump();
        check(registry.activeRoot(0) == null, "background A cannot activate WMS root");
        fact(rootB, PID_B, UID_B, DesktopRootRegistry.ACTIVATED, 12, 1, true);
        runUntilActive(registry);
        check(registry.activeRoot(0) != null && registry.focusedWindow(0) != null,
                "matched B activation selects a WMS window");
        check(registry.focusedWindow(0).channelIncarnation != null
                && pendingFocus(registry, channelB, true),
                "matched activation emits original B focus lease");

        // A late predecessor resign cannot clear the newer B activation.
        fact(rootA, PID_A, UID_A, DesktopRootRegistry.RESIGNED, 11, 2, false);
        pump();
        check(registry.activeRoot(0) != null && registry.focusedWindow(0) != null,
                "late A resign cannot clear B");

        // A newer A activation fact is also rejected while B is freshly
        // foreground; it must not enqueue an original A focus gain.
        fact(rootA, PID_A, UID_A, DesktopRootRegistry.ACTIVATED, 11, 3, true);
        pump();
        check(registry.activeRoot(0) != null && registry.focusedWindow(0) != null
                && !pendingFocus(registry, channelA, true),
                "newer background A activation cannot displace B");

        // A same-root activation followed immediately by resign leaves no
        // selected winner even while its retained publication work is queued.
        fixture.foreground.foregroundPid = PID_A;
        fact(rootA, PID_A, UID_A, DesktopRootRegistry.ACTIVATED, 11, 4, true);
        fact(rootA, PID_A, UID_A, DesktopRootRegistry.RESIGNED, 11, 5, false);
        pump();
        check(registry.focusedWindow(0) == null,
                "activation then resign clears selection before handler drain");

        // CLOSED unbinds every exact original channel and clears the selected
        // root, including the original root/channel pairing.
        fixture.foreground.foregroundPid = PID_B;
        fact(rootB, PID_B, UID_B, DesktopRootRegistry.ACTIVATED, 12, 2, true);
        runUntilActive(registry);
        check(registry.focusedWindow(0) != null, "B reactivation selected window");
        fact(rootB, PID_B, UID_B, DesktopRootRegistry.CLOSED, 12, 3, false);
        check(registry.focusedWindow(0) == null && spec(registry, windowB).rootToken == null
                && spec(registry, windowB).rootIncarnation == null,
                "exact CLOSED clears activation and original root pair");
        pump();
    }

    private static void testLaterBindAndFreshForeground() throws Exception {
        DesktopWindowMetadataRegistry.reset();
        WindowInputPublisher.reset();
        Fixture fixture = new Fixture(PID_A);
        fixture.attach(PID_A, UID_A, new Binder());
        WindowSessionEndpoint session = fixture.openSession(PID_A, UID_A, new Binder());
        Binder firstWindow = new Binder();
        Binder.setCallingIdentity(PID_A, UID_A);
        add(session, firstWindow);
        InputChannel first = channel(session, firstWindow);
        Binder lifetime = new Binder();
        IBinder root = register(fixture, PID_A, UID_A, 21, lifetime);
        bind(root, first, PID_A, UID_A);
        fact(root, PID_A, UID_A, DesktopRootRegistry.ACTIVATED, 21, 1, true);
        WindowFocusRegistry registry = registry(fixture);
        runUntilActive(registry);
        check(registry.focusedWindow(0) != null, "first matched activation selected");

        // Foreground changed before a later bind: that bind must not grant the
        // stale activation to a newly added original channel.
        fixture.foreground.foregroundPid = PID_B;
        relayout(session, firstWindow);
        check(registry.focusedWindow(0) == null,
                "foreground change before relayout revokes stale focus");
        Binder secondWindow = new Binder();
        add(session, secondWindow);
        InputChannel second = channel(session, secondWindow);
        bind(root, second, PID_A, UID_A);
        check(registry.focusedWindow(0) == null,
                "later bind does not blindly overwrite selection");
        check(!pendingFocus(registry, second, true),
                "fresh foreground change blocks stale later-bind gain");
    }

    private static void testActivationBeforeBindAndClosedWithoutWindows() throws Exception {
        DesktopWindowMetadataRegistry.reset();
        WindowInputPublisher.reset();
        Fixture fixture = new Fixture(PID_A);
        fixture.attach(PID_A, UID_A, new Binder());
        Binder lifetime = new Binder();
        IBinder root = register(fixture, PID_A, UID_A, 31, lifetime);
        fact(root, PID_A, UID_A, DesktopRootRegistry.ACTIVATED, 31, 1, true);
        WindowFocusRegistry registry = registry(fixture);
        runUntilActive(registry);
        check(registry.activeRoot(0) != null && registry.focusedWindow(0) == null,
                "activation before canonical window bind retains root only");

        WindowSessionEndpoint session = fixture.openSession(PID_A, UID_A, new Binder());
        Binder window = new Binder();
        Binder.setCallingIdentity(PID_A, UID_A);
        add(session, window);
        InputChannel original = channel(session, window);
        bind(root, original, PID_A, UID_A);
        pump();
        check(registry.focusedWindow(0) != null
                        && registry.focusedWindow(0).channelIncarnation instanceof WindowInputEndpoint
                        && ((WindowInputEndpoint) registry.focusedWindow(0).channelIncarnation)
                                .owns(original),
                "later canonical bind selects original window under same root");

        // CLOSED must also retire a root with no bound windows; unbind cannot
        // rely on finding at least one entry to trigger policy retirement.
        Fixture noWindow = new Fixture(PID_A);
        noWindow.attach(PID_A, UID_A, new Binder());
        Binder noWindowLifetime = new Binder();
        IBinder noWindowRoot = register(noWindow, PID_A, UID_A, 32, noWindowLifetime);
        fact(noWindowRoot, PID_A, UID_A, DesktopRootRegistry.ACTIVATED, 32, 1, true);
        WindowFocusRegistry noWindowRegistry = registry(noWindow);
        runUntilActive(noWindowRegistry);
        check(noWindowRegistry.activeRoot(0) != null,
                "no-window activation retains active root");
        fact(noWindowRoot, PID_A, UID_A, DesktopRootRegistry.CLOSED, 32, 2, false);
        check(noWindowRegistry.activeRoot(0) == null,
                "CLOSED retires no-window active root before empty unbind");
    }

    private static void testForegroundQueryRace(int terminalKind) throws Exception {
        DesktopWindowMetadataRegistry.reset();
        WindowInputPublisher.reset();
        Fixture fixture = new Fixture(PID_A);
        WindowSessionEndpoint session = fixture.session(PID_A, UID_A, new Binder());
        Binder window = new Binder();
        Binder.setCallingIdentity(PID_A, UID_A);
        add(session, window);
        InputChannel original = channel(session, window);
        Binder lifetime = new Binder();
        long incarnation = 41 + terminalKind;
        IBinder root = register(fixture, PID_A, UID_A, incarnation, lifetime);
        bind(root, original, PID_A, UID_A);
        fixture.foreground.onQuery = () -> {
            try {
                fact(root, PID_A, UID_A, terminalKind, incarnation, 2,
                        terminalKind != DesktopRootRegistry.RESIGNED);
            } catch (Exception error) {
                throw new AssertionError(error);
            }
        };
        fact(root, PID_A, UID_A, DesktopRootRegistry.ACTIVATED, incarnation, 1, true);
        WindowFocusRegistry registry = registry(fixture);
        for (int i = 0; i < 64 && fixture.foreground.lastQuery == null; ++i) {
            check(android.os.HandlerThread.runLatestNext(), "foreground race handler work missing");
        }
        check(fixture.foreground.lastQuery == fixture.foreground.lastCapture,
                "foreground race queried pinned process instance");
        check(registry.activeRoot(0) == null && registry.focusedWindow(0) == null
                && !pendingFocus(registry, original, true),
                "post-query terminal/resign fact blocks stale activation gain");
        pump();
    }

    private static void testDriverFailureStopsQueuedWork() throws Exception {
        DesktopWindowMetadataRegistry.reset();
        WindowInputPublisher.reset();
        Fixture fixture = new Fixture(PID_A);
        WindowSessionEndpoint session = fixture.session(PID_A, UID_A, new Binder());
        Binder window = new Binder();
        Binder.setCallingIdentity(PID_A, UID_A);
        add(session, window);
        InputChannel original = channel(session, window);
        WindowFocusRegistry registry = registry(fixture);
        WindowPublicationDriver driver = driver(fixture);
        final int[] counter = {0};
        driver.executePolicy(() -> { throw new AssertionError("queued policy failure"); });
        driver.executePolicy(() -> ++counter[0]);
        remove(session, window, PID_A, UID_A);
        WindowFocusRegistry.PublicationBatch retained = registry.pendingBatch();
        check(retained != null, "remove retains a publication batch before driver turn");
        int disposed = InputChannel.disposedCount();
        check(android.os.HandlerThread.runLatestNext(), "first queued policy missing");
        check(android.os.HandlerThread.runLatestNext(), "second queued policy missing");
        check(android.os.HandlerThread.runLatestNext(), "queued driver turn missing");
        check(counter[0] == 0, "queued policy callback ran after driver failure");
        check(registry.pendingBatch() == retained,
                "failed driver turn did not acknowledge the retained batch");
        check(driver.owns(original),
                "failed driver turn retained ownership of original endpoint");
        check(InputChannel.disposedCount() == disposed,
                "failed driver turn did not dispose original input channel");
        check(field(driver, "failure") instanceof AssertionError,
                "driver retained original queued failure");
    }

    public static void main(String[] args) throws Exception {
        testProviderFalseAndBirthPinned();
        testActivationSelectionAndLateResign();
        testLaterBindAndFreshForeground();
        testActivationBeforeBindAndClosedWithoutWindows();
        testForegroundQueryRace(DesktopRootRegistry.RESIGNED);
        testForegroundQueryRace(DesktopRootRegistry.CLOSED);
        testDriverFailureStopsQueuedWork();
        System.out.println("WMS desktop-root activation/fresh-foreground checks=" + checks);
    }
}
