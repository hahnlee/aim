package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.view.InputChannel;
import android.view.IWindowManager;
import android.view.IWindowSession;
import android.view.View;
import android.view.WindowManager;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import java.lang.reflect.Field;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** Actual authenticated Session/root-capability join with test-only platform IO. */
public final class DesktopRootWindowBindingTest {
    private static int checks;
    private static final DesktopForegroundAuthority.Provider HOST =
            new DesktopForegroundAuthority.Provider() {
                @Override public DesktopForegroundAuthority.ProcessInstance capture(int pid) {
                    return new DesktopForegroundAuthority.ProcessInstance(pid, 1, 0);
                }
                @Override public boolean isForeground(DesktopForegroundAuthority.ProcessInstance process) {
                    throw new AssertionError("binding must not request a foreground grant");
                }
            };
    private static void check(boolean value, String message) {
        ++checks;
        if (!value) throw new AssertionError(message);
    }
    private static Object field(Object owner, String name) throws Exception {
        Field value = owner.getClass().getDeclaredField(name);
        value.setAccessible(true);
        return value.get(owner);
    }
    private static void rejected(IBinder capability, IBinder channel, int display) throws Exception {
        ++checks;
        try {
            DesktopRootProtocol.bindWindow(capability, channel, display);
            throw new AssertionError("invalid binding accepted");
        } catch (DesktopRootProtocol.BindingRejectedException | RuntimeException expected) {
            // Production authority rejects the request; no success stub.
        }
    }
    private static WindowSessionEndpoint session(WindowManagerEndpoint manager) throws Exception {
        Parcel data = Parcel.obtain(), reply = Parcel.obtain();
        data.writeInterfaceToken("android.view.IWindowManager");
        data.writeStrongBinder(new Binder());
        manager.transact(IWindowManager.Stub.TRANSACTION_openSession, data, reply, 0);
        reply.readException();
        return (WindowSessionEndpoint) reply.readStrongBinder();
    }
    private static void add(WindowSessionEndpoint session, IBinder window) throws Exception {
        WindowManager.LayoutParams attrs = new WindowManager.LayoutParams();
        attrs.type = 1;
        attrs.flags = 0x20;
        attrs.x = 7;
        attrs.y = 11;
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken("android.view.IWindowSession");
        data.writeStrongBinder(window);
        data.writeTypedObject(attrs, 0);
        data.writeInt(View.VISIBLE);
        data.writeInt(0); data.writeInt(0); data.writeInt(0); data.writeInt(1);
        session.transact(IWindowSession.Stub.TRANSACTION_addToDisplayAsUser, data, Parcel.obtain(), 0);
        data = Parcel.obtain();
        data.writeInterfaceToken("android.view.IWindowSession");
        data.writeStrongBinder(window);
        data.writeTypedObject(attrs, 0);
        data.writeInt(200); data.writeInt(100); data.writeInt(View.VISIBLE);
        data.writeInt(0); data.writeInt(0); data.writeInt(0);
        session.transact(IWindowSession.Stub.TRANSACTION_relayout, data, Parcel.obtain(), 0);
    }
    @SuppressWarnings("unchecked")
    private static InputChannel channel(WindowSessionEndpoint session) throws Exception {
        Map<Object, InputChannel> channels = (Map<Object, InputChannel>) field(session, "serverInputChannels");
        return channels.values().iterator().next();
    }
    @SuppressWarnings("unchecked")
    private static InputChannel channel(WindowSessionEndpoint session, IBinder window) throws Exception {
        Map<WindowSessionWindowOwnership.Registration, InputChannel> channels =
                (Map<WindowSessionWindowOwnership.Registration, InputChannel>) field(session, "serverInputChannels");
        for (Map.Entry<WindowSessionWindowOwnership.Registration, InputChannel> entry : channels.entrySet())
            if (entry.getKey().window().equals(window)) return entry.getValue();
        throw new AssertionError("missing exact window channel");
    }
    private static void remove(WindowSessionEndpoint session, IBinder window) throws Exception {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken("android.view.IWindowSession");
        data.writeStrongBinder(window);
        session.transact(IWindowSession.Stub.TRANSACTION_remove, data, Parcel.obtain(), 0);
    }
    @SuppressWarnings("unchecked")
    private static WindowFocusRegistry.WindowSpec onlySpec(WindowManagerEndpoint manager) throws Exception {
        Object registry = field(field(manager, "publications"), "registry");
        Map<Object, Object> windows = (Map<Object, Object>) field(registry, "windows");
        return (WindowFocusRegistry.WindowSpec) field(windows.values().iterator().next(), "spec");
    }
    @SuppressWarnings("unchecked")
    private static WindowFocusRegistry.WindowSpec spec(WindowManagerEndpoint manager, IBinder window) throws Exception {
        Object registry = field(field(manager, "publications"), "registry");
        Map<Object, Object> windows = (Map<Object, Object>) field(registry, "windows");
        for (Object stored : windows.values()) {
            WindowFocusRegistry.WindowSpec spec = (WindowFocusRegistry.WindowSpec) field(stored, "spec");
            if (((WindowSessionWindowOwnership.Registration) spec.windowToken).window().equals(window)) return spec;
        }
        throw new AssertionError("missing canonical window snapshot");
    }
    private static void closed(IBinder capability, long incarnation) throws Exception {
        DesktopRootProtocol.fact(capability, DesktopRootRegistry.CLOSED, incarnation, 1, false);
    }

    private static void closeBeforeDelayedBind(ApplicationProcessRegistry processes,
            WindowManagerEndpoint manager, IBinder channel) throws Exception {
        final DesktopRootWindowBindingOwner actual =
                (DesktopRootWindowBindingOwner) field(manager, "publications");
        final CountDownLatch admitted = new CountDownLatch(1), resume = new CountDownLatch(1);
        DesktopRootWindowBindingOwner delayed = new DesktopRootWindowBindingOwner() {
            @Override public void bind(DesktopRootRegistry.Registration root, IBinder token, int display) {
                admitted.countDown();
                try {
                    if (!resume.await(5, TimeUnit.SECONDS)) throw new AssertionError("bind release timeout");
                } catch (InterruptedException error) { throw new AssertionError(error); }
                actual.bind(root, token, display);
            }
            @Override public void unbind(DesktopRootRegistry.Registration root) { actual.unbind(root); }
            @Override public void factChanged(DesktopRootRegistry.Registration root,
                    DesktopRootRegistry.Fact fact) { actual.factChanged(root, fact); }
        };
        DesktopRootEndpoint endpoint = new DesktopRootEndpoint(processes, new DesktopRootRegistry(delayed, HOST));
        IBinder capability = DesktopRootProtocol.register(endpoint, 20, new Binder());
        AtomicReference<Throwable> result = new AtomicReference<>();
        Thread sender = new Thread(() -> {
            Binder.setCallingIdentity(4101, 10101);
            try { DesktopRootProtocol.bindWindow(capability, channel, 0); }
            catch (Throwable failure) { result.set(failure); }
        }, "delayed-window-bind");
        sender.start();
        check(admitted.await(5, TimeUnit.SECONDS), "bind admitted outside root registry monitor");
        try {
            closed(capability, 20);
            check(onlySpec(manager).rootToken == null, "CLOSED commits before delayed owner invocation");
        } finally { resume.countDown(); }
        sender.join(5000);
        check(!sender.isAlive(), "delayed binding returns after close");
        check(result.get() instanceof DesktopRootProtocol.BindingRejectedException,
                "late admitted binding is terminally rejected");
        check(onlySpec(manager).rootToken == null, "late binding cannot resurrect closed root");
    }

    public static void main(String[] args) throws Exception {
        WindowInputPublisher.reset();
        ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        Binder thread = new Binder();
        processes.beginAttachment(4101, 10101, thread, 1);
        processes.identify(4101, 1, thread, "fixture.binding");
        Binder.setCallingIdentity(4101, 10101);
        WindowManagerEndpoint manager = new WindowManagerEndpoint(processes, new DesktopWindowMetadataRegistry());
        DesktopRootEndpoint roots = new DesktopRootEndpoint(processes, manager.createDesktopRootRegistry(HOST));
        WindowSessionEndpoint session = session(manager);
        Binder window = new Binder();
        add(session, window); // Canonical ADD precedes native root registration.
        InputChannel original = channel(session);
        WindowFocusRegistry.WindowSpec before = onlySpec(manager);
        check(before.rootToken == null && before.rootIncarnation == null, "ADD remains unbound");
        Binder lifetime = new Binder();
        DesktopForegroundAuthority.Provider wrongProcess = new DesktopForegroundAuthority.Provider() {
            @Override public DesktopForegroundAuthority.ProcessInstance capture(int pid) {
                return new DesktopForegroundAuthority.ProcessInstance(pid + 1, 1, 0);
            }
            @Override public boolean isForeground(DesktopForegroundAuthority.ProcessInstance process) {
                throw new AssertionError("rejected capture cannot request focus");
            }
        };
        try {
            DesktopRootProtocol.register(new DesktopRootEndpoint(processes,
                    manager.createDesktopRootRegistry(wrongProcess)), 16, new Binder());
            throw new AssertionError("foreign host process snapshot accepted");
        } catch (SecurityException expected) {
            check(before.rootToken == null, "invalid provider capture does not publish a root");
        }
        IBinder capability = DesktopRootProtocol.register(roots, 17, lifetime);
        DesktopRootProtocol.bindWindow(capability, original.getToken(), 0);
        WindowFocusRegistry.WindowSpec bound = onlySpec(manager);
        check(bound.rootToken != null && bound.rootIncarnation != null, "exact original channel binds root");
        DesktopRootRegistry.Registration capturedRoot = (DesktopRootRegistry.Registration) bound.rootToken;
        check(capturedRoot.hostProcess != null && capturedRoot.hostProcess.pid == 4101
                && capturedRoot.hostProcess.startSeconds == 1,
                "authenticated REGISTER captures immutable provider process instance");
        check(bound.windowToken == before.windowToken && bound.channelIncarnation == before.channelIncarnation
                && bound.flags == before.flags && bound.visible == before.visible
                && bound.left == before.left && bound.top == before.top
                && bound.right == before.right && bound.bottom == before.bottom
                && bound.order == before.order, "binding preserves latest geometry/attrs/identity/order");
        WindowFocusRegistry registry = (WindowFocusRegistry) field(field(manager, "publications"), "registry");
        check(registry.activeRoot(0) == null && registry.focusedWindow(0) == null, "binding grants no activation/focus");
        DesktopRootProtocol.bindWindow(capability, original.getToken(), 0);
        check(onlySpec(manager).rootToken == bound.rootToken, "same tuple is idempotent");
        rejected(capability, new Binder(), 0);
        rejected(capability, original.getToken(), 1);
        Binder.setCallingIdentity(4101, 10102);
        rejected(capability, original.getToken(), 0);
        Binder.setCallingIdentity(4101, 10101);
        IBinder otherRoot = DesktopRootProtocol.register(roots, 18, new Binder());
        rejected(otherRoot, original.getToken(), 0);
        Binder secondWindow = new Binder();
        add(session, secondWindow);
        DesktopRootProtocol.bindWindow(capability, channel(session, secondWindow).getToken(), 0);
        check(spec(manager, secondWindow).rootToken == bound.rootToken,
                "later canonical window joins the same exact root");
        closed(capability, 17);
        check(spec(manager, window).rootToken == null && spec(manager, window).rootIncarnation == null,
                "CLOSED invalidates original binding without retiring WMS window");
        check(spec(manager, secondWindow).rootToken == null
                && spec(manager, secondWindow).rootIncarnation == null,
                "CLOSED invalidates every independently bound window");
        remove(session, secondWindow);
        rejected(capability, original.getToken(), 0);
        DesktopRootProtocol.bindWindow(otherRoot, original.getToken(), 0);
        remove(session, window);
        add(session, window); // Same IWindow, a distinct actual channel incarnation.
        InputChannel successor = channel(session);
        check(!original.getToken().equals(successor.getToken()), "successor uses a distinct channel");
        rejected(otherRoot, original.getToken(), 0);
        DesktopRootProtocol.bindWindow(otherRoot, successor.getToken(), 0);
        closed(otherRoot, 18);
        Binder deathLifetime = new Binder();
        IBinder deathRoot = DesktopRootProtocol.register(roots, 19, deathLifetime);
        DesktopRootProtocol.bindWindow(deathRoot, successor.getToken(), 0);
        deathLifetime.die();
        check(onlySpec(manager).rootToken == null, "exact client death invalidates binding");
        rejected(deathRoot, successor.getToken(), 0);
        closeBeforeDelayedBind(processes, manager, successor.getToken());
        IBinder staleRoot = DesktopRootProtocol.register(roots, 21, new Binder());
        DesktopRootProtocol.bindWindow(staleRoot, successor.getToken(), 0);
        Binder nextThread = new Binder();
        processes.retireAttached(4101, 1, thread);
        processes.beginAttachment(4101, 10101, nextThread, 2);
        processes.identify(4101, 2, nextThread, "fixture.binding");
        rejected(staleRoot, successor.getToken(), 0);
        check(onlySpec(manager).rootToken == null,
                "pre-bind stale own attachment retires prior window binding");
        System.out.println("actual WMS/root-capability binding checks=" + checks);
    }
}
