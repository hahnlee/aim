package dev.darwinart.runtime.am;

import android.app.IServiceConnection;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Binder;
import android.os.IBinder;
import android.os.RemoteException;
import java.util.ArrayList;

/** Focused ownership and detached-batch coverage for ServiceConnectionIndex. */
public final class ServiceConnectionIndexTest {
    private static final ComponentName COMPONENT = new ComponentName("example", "example.Service");

    private static final class Connection extends Binder implements IServiceConnection {
        @Override public IBinder asBinder() { return this; }
        @Override public void connected(ComponentName name, IBinder service, boolean dead)
                throws RemoteException {}
    }

    private static final class Fixture {
        final ServiceRecord service = new ServiceRecord(
                COMPONENT, null, "example", 10042, false, new ServiceInfo());
        final IntentBindRecord first = binding(1);
        final IntentBindRecord second = binding(2);

        private IntentBindRecord binding(long sequence) {
            return new IntentBindRecord(
                    service,
                    new Intent().setComponent(COMPONENT),
                    null,
                    sequence);
        }
    }

    private static ApplicationProcessRegistry.AttachedApplication application(
            int pid, long sequence, int uid, IBinder thread) {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        registry.beginAttachment(pid, uid, thread, sequence);
        registry.identify(pid, sequence, thread, "example");
        return registry.finishAttachment(pid, sequence);
    }

    private static ConnectionRecord record(
            Connection endpoint, IntentBindRecord binding, ServiceConnectionOwner owner, int uid) {
        return new ConnectionRecord(endpoint, binding, 0, uid, owner);
    }

    public static void main(String[] args) {
        exactOwnerMatchingLeavesSystemRecordAndDeathPinUntilFinalDetach();
        bindingDetachUpdatesBothLedgersAndIsIdempotent();
        System.out.println("service connection index: PASS exact owner/death/binding/detach");
    }

    private static void exactOwnerMatchingLeavesSystemRecordAndDeathPinUntilFinalDetach() {
        Fixture fixture = new Fixture();
        ServiceConnectionIndex index = new ServiceConnectionIndex();
        Connection endpoint = new Connection();
        IBinder oldThread = new Binder();
        ApplicationProcessRegistry.AttachedApplication attached =
                application(41, 7, 10042, oldThread);
        ServiceConnectionOwner appOwner = ServiceConnectionOwner.application(attached);
        ConnectionRecord appRecord = record(endpoint, fixture.first, appOwner, 10042);
        ConnectionRecord systemRecord = new ConnectionRecord(
                endpoint, fixture.second, 0, android.os.Process.SYSTEM_UID,
                ServiceConnectionOwner.system());
        IBinder.DeathRecipient death = () -> {};

        index.setDeath(endpoint, death);
        index.add(appRecord);
        index.add(appRecord); // A repeated publication cannot duplicate a record.
        index.add(systemRecord);
        ArrayList<ConnectionRecord> snapshot = index.records(endpoint);
        check(snapshot.size() == 2);
        snapshot.clear();
        check(index.records(endpoint).size() == 2);

        // Same PID with a new process incarnation must not detach the old app.
        ApplicationProcessRegistry.AttachedApplication replacement =
                application(41, 8, 10042, new Binder());
        check(index.detachClient(replacement).isEmpty());
        check(index.contains(appRecord));

        ServiceConnectionIndex.Detached appDetached = index.detachClient(attached);
        check(appDetached.records().size() == 1);
        check(appDetached.deaths().isEmpty());
        check(!index.contains(appRecord));
        check(index.contains(systemRecord));
        check(fixture.first.connections.isEmpty());
        check(fixture.second.connections.size() == 1);
        check(index.detachClient(attached).isEmpty());

        // The shared death pin is taken only after the final indexed record.
        ServiceConnectionIndex.Detached finalDetached = index.detachBinder(endpoint);
        check(finalDetached.records().size() == 1);
        check(finalDetached.deaths().size() == 1);
        check(finalDetached.deaths().get(0).binder == endpoint);
        check(finalDetached.deaths().get(0).recipient == death);
        check(fixture.second.connections.isEmpty());
        check(index.records(endpoint).isEmpty());
        check(index.death(endpoint) == null);
        check(index.detachBinder(endpoint).isEmpty());
    }

    private static void bindingDetachUpdatesBothLedgersAndIsIdempotent() {
        Fixture fixture = new Fixture();
        ServiceConnectionIndex index = new ServiceConnectionIndex();
        Connection firstEndpoint = new Connection();
        Connection secondEndpoint = new Connection();
        ConnectionRecord first = record(
                firstEndpoint,
                fixture.first,
                ServiceConnectionOwner.system(),
                android.os.Process.SYSTEM_UID);
        ConnectionRecord second = record(
                secondEndpoint,
                fixture.first,
                ServiceConnectionOwner.system(),
                android.os.Process.SYSTEM_UID);
        IBinder.DeathRecipient firstDeath = () -> {};
        IBinder.DeathRecipient secondDeath = () -> {};
        index.setDeath(firstEndpoint, firstDeath);
        index.setDeath(secondEndpoint, secondDeath);
        index.add(first);
        index.add(second);
        check(fixture.first.connections.size() == 2);

        ServiceConnectionIndex.Detached detached = index.detachBinding(fixture.first);
        check(detached.records().size() == 2);
        check(detached.deaths().size() == 2);
        check(fixture.first.connections.isEmpty());
        check(index.records(firstEndpoint).isEmpty());
        check(index.records(secondEndpoint).isEmpty());
        check(index.death(firstEndpoint) == null && index.death(secondEndpoint) == null);
        check(index.detachBinding(fixture.first).isEmpty());
    }

    private static void check(boolean condition) {
        if (!condition) throw new AssertionError();
    }
}
