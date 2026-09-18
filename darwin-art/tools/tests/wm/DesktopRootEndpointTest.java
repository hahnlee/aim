package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/** Component coverage for the actual desktop-root registry and Binder owners. */
public final class DesktopRootEndpointTest {
    private static final int PID = 6101;
    private static final int UID = 106101;

    private static class Lifetime extends Binder {}

    private static final class ImmediateLifetime extends Lifetime {
        @Override public void linkToDeath(DeathRecipient recipient, int flags) {
            recipient.binderDied();
        }
    }

    private static IBinder thread() { return new Lifetime(); }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void expectFailure(Throwing operation, String message) throws Exception {
        try {
            operation.run();
            throw new AssertionError(message);
        } catch (IllegalArgumentException | IllegalStateException | SecurityException expected) {
            // Root authentication and malformed transport both fail closed.
        }
    }

    private interface Throwing { void run() throws Exception; }

    private static ApplicationProcessRegistry processes(IBinder appThread) {
        ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        processes.beginAttachment(PID, UID, appThread, 1L);
        processes.identify(PID, 1L, appThread, "org.example.root");
        processes.finishAttachment(PID, 1L);
        return processes;
    }

    private static Parcel registerRequest(long incarnation, IBinder lifetime) {
        Parcel request = Parcel.obtain();
        request.writeInterfaceToken(DesktopRootEndpoint.DESCRIPTOR);
        request.writeLong(incarnation);
        request.writeStrongBinder(lifetime);
        return request;
    }

    private static Parcel factRequest(int kind, long incarnation, long serial, boolean key) {
        Parcel request = Parcel.obtain();
        request.writeInterfaceToken(DesktopRootEndpoint.DESCRIPTOR);
        request.writeInt(kind);
        request.writeLong(incarnation);
        request.writeLong(serial);
        request.writeBoolean(key);
        return request;
    }

    private static IBinder register(DesktopRootEndpoint endpoint, long incarnation,
            Lifetime lifetime) throws Exception {
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(DesktopRootEndpoint.TRANSACTION_REGISTER,
                registerRequest(incarnation, lifetime), reply, 0), "register rejected");
        reply.readException();
        return reply.readStrongBinder();
    }

    private static boolean fact(IBinder capability, int kind, long incarnation, long serial,
            boolean key) throws Exception {
        Parcel reply = Parcel.obtain();
        check(capability.transact(DesktopRootEndpoint.TRANSACTION_FACT,
                factRequest(kind, incarnation, serial, key), reply, 0), "fact rejected");
        reply.readException();
        return reply.readBoolean();
    }

    private static void testUnsignedFactsAndTerminalClose() throws Exception {
        IBinder appThread = thread();
        ApplicationProcessRegistry processes = processes(appThread);
        DesktopRootRegistry registry = new DesktopRootRegistry();
        DesktopRootEndpoint endpoint = new DesktopRootEndpoint(processes, registry);
        Binder.setCallingIdentity(PID, UID);
        Lifetime lifetime = new Lifetime();
        long incarnation = Long.MIN_VALUE;
        IBinder capability = register(endpoint, incarnation, lifetime);
        check(fact(capability, DesktopRootRegistry.ACTIVATED, incarnation, Long.MIN_VALUE, true),
                "negative unsigned activated fact was not retained");
        check(fact(capability, DesktopRootRegistry.RESIGNED, incarnation, -1L, false),
                "unsigned serial ordering was signed");
        expectFailure(() -> fact(capability, DesktopRootRegistry.CLOSED, incarnation, Long.MIN_VALUE, true),
                "stale unsigned serial accepted");
        Lifetime closeLifetime = new Lifetime();
        long closeIncarnation = 11L;
        IBinder closeCapability = register(endpoint, closeIncarnation, closeLifetime);
        check(fact(closeCapability, DesktopRootRegistry.CLOSED, closeIncarnation, -1L, true),
                "closed fact was not acknowledged");
        check(registry.registrationCount() == 1, "terminal close retained registration");
        expectFailure(() -> register(endpoint, closeIncarnation, new Lifetime()),
                "closed root incarnation was reopened by same attachment");
        expectFailure(() -> fact(closeCapability, DesktopRootRegistry.RESIGNED, closeIncarnation, 1L, false),
                "stale capability remained usable after close");
        lifetime.dieForTest();
        check(registry.registrationCount() == 0, "death did not remove open registration");
    }

    private static void testValidationAndDuplicates() throws Exception {
        IBinder firstThread = thread();
        ApplicationProcessRegistry processes = processes(firstThread);
        DesktopRootRegistry registry = new DesktopRootRegistry();
        DesktopRootEndpoint endpoint = new DesktopRootEndpoint(processes, registry);
        Binder.setCallingIdentity(PID, UID);
        Lifetime lifetime = new Lifetime();
        long incarnation = 9L;
        IBinder capability = register(endpoint, incarnation, lifetime);
        expectFailure(() -> fact(capability, DesktopRootRegistry.ACTIVATED, incarnation, 1L, false),
                "activated non-key fact accepted");
        expectFailure(() -> fact(capability, DesktopRootRegistry.RESIGNED, incarnation, 1L, true),
                "resigned key fact accepted");
        expectFailure(() -> fact(capability, DesktopRootRegistry.ACTIVATED, incarnation, 0L, true),
                "zero serial accepted");
        Parcel trailing = factRequest(DesktopRootRegistry.ACTIVATED, incarnation, 1L, true);
        trailing.writeInt(17);
        expectFailure(() -> capability.transact(DesktopRootEndpoint.TRANSACTION_FACT, trailing,
                Parcel.obtain(), 0), "trailing fact data accepted");
        expectFailure(() -> register(endpoint, incarnation, new Lifetime()),
                "duplicate root registration silently replaced");
        // Root incarnations are scoped by the pinned application attachment;
        // another live process may legitimately use the same numeric value.
        IBinder secondThread = thread();
        processes.beginAttachment(PID + 1, UID + 1, secondThread, 2L);
        processes.identify(PID + 1, 2L, secondThread, "org.example.other");
        processes.finishAttachment(PID + 1, 2L);
        Binder.setCallingIdentity(PID + 1, UID + 1);
        Lifetime secondLifetime = new Lifetime();
        register(endpoint, incarnation, secondLifetime);
        check(registry.registrationCount() == 2, "cross-process root incarnation collided");
        lifetime.dieForTest();
        check(registry.registrationCount() == 1, "death removed another attachment registration");
        secondLifetime.dieForTest();
        check(registry.registrationCount() == 0, "death did not remove exact registration");
    }

    private static void testDeadCallerRemovesExactRegistration() throws Exception {
        IBinder originalThread = thread();
        ApplicationProcessRegistry processes = processes(originalThread);
        DesktopRootRegistry registry = new DesktopRootRegistry();
        DesktopRootEndpoint endpoint = new DesktopRootEndpoint(processes, registry);
        Binder.setCallingIdentity(PID, UID);
        Lifetime lifetime = new Lifetime();
        IBinder capability = register(endpoint, 10L, lifetime);
        Binder.setCallingIdentity(PID + 1, UID + 1);
        expectFailure(() -> fact(capability, DesktopRootRegistry.ACTIVATED, 10L, 1L, true),
                "dead/foreign caller accepted fact");
        check(registry.registrationCount() == 1, "foreign caller revoked owner registration");
        Binder.setCallingIdentity(PID, UID);
        processes.retireAttached(PID, 1L, originalThread);
        processes.beginAttachment(PID, UID, thread(), 2L);
        expectFailure(() -> fact(capability, DesktopRootRegistry.ACTIVATED, 10L, 1L, true),
                "recycled attachment accepted fact");
        check(registry.registrationCount() == 0, "stale attachment did not remove exact registration");
    }

    private static void testImmediateDeathNeverPublishes() throws Exception {
        ApplicationProcessRegistry processes = processes(thread());
        DesktopRootRegistry registry = new DesktopRootRegistry();
        DesktopRootEndpoint endpoint = new DesktopRootEndpoint(processes, registry);
        Binder.setCallingIdentity(PID, UID);
        expectFailure(() -> register(endpoint, 13L, new ImmediateLifetime()),
                "immediately dead lifetime received a capability");
        check(registry.registrationCount() == 0, "immediate death left a pending registration");
    }

    private static void testRetirementBetweenPrepareAndPublish() throws Exception {
        ApplicationProcessRegistry processes = processes(thread());
        DesktopRootRegistry registry = new DesktopRootRegistry();
        DesktopRootEndpoint endpoint = new DesktopRootEndpoint(processes, registry);
        Binder.setCallingIdentity(PID, UID);
        WindowSessionIdentity identity = new WindowSessionIdentity(processes, PID, UID);
        DesktopRootRegistry.Registration prepared = registry.prepare(identity, 20L, new Lifetime());
        IBinder current = register(endpoint, 20L, new Lifetime());
        fact(current, DesktopRootRegistry.CLOSED, 20L, 1L, false);
        expectFailure(() -> registry.linkAndPublish(prepared), "prepared retired root reopened");
        Lifetime retiringDuringLink = new Lifetime() {
            @Override public void linkToDeath(DeathRecipient recipient, int flags)
                    throws android.os.RemoteException {
                super.linkToDeath(recipient, flags);
                check(!Thread.holdsLock(registry), "Binder link holds registry monitor");
                try {
                    IBinder newer = register(endpoint, 22L, new Lifetime());
                    fact(newer, DesktopRootRegistry.CLOSED, 22L, 1L, false);
                } catch (Exception failure) {
                    throw new AssertionError(failure);
                }
            }
        };
        expectFailure(() -> register(endpoint, 21L, retiringDuringLink),
                "pending registration ignored newer retirement");
        check(registry.registrationCount() == 0, "rejected pending registration survived");
    }

    public static void main(String[] args) throws Exception {
        testUnsignedFactsAndTerminalClose();
        testValidationAndDuplicates();
        testDeadCallerRemovesExactRegistration();
        testImmediateDeathNeverPublishes();
        testRetirementBetweenPrepareAndPublish();
        System.out.println("Desktop root registration, authenticated facts, and death cleanup: PASS");
    }
}
