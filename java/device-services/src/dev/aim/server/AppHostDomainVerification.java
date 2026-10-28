package dev.aim.server;

import android.content.ComponentName;
import android.content.Context;
import android.content.pm.verify.domain.DomainSet;
import android.content.pm.verify.domain.DomainVerificationInfo;
import android.os.RemoteException;
import com.android.server.SystemConfig;
import com.android.server.compat.PlatformCompat;
import com.android.server.pm.Computer;
import com.android.server.pm.verify.domain.DomainVerificationEnforcer;
import com.android.server.pm.verify.domain.DomainVerificationManagerInternal;
import com.android.server.pm.verify.domain.DomainVerificationManagerStub;
import com.android.server.pm.verify.domain.DomainVerificationService;
import com.android.server.pm.verify.domain.proxy.DomainVerificationProxy;
import java.util.List;
import java.util.Set;
import java.util.UUID;

/**
 * Non-hot verifier API: retain android-16.0.0_r1's Stub, public status validation
 * and Enforcer. The existing native package owner owns the state and disk writes.
 * This adapter supplies caller identity only through the system-UID bridge.
 */
final class AppHostDomainVerification extends IAppHostDomainVerification.Stub {
    private final ThreadLocal<Integer> caller = new ThreadLocal<>();
    private final DomainVerificationManagerStub entry;

    AppHostDomainVerification(Context context, ISystemProcess host) throws RemoteException {
        IAppHostDomainVerification backend = IAppHostDomainVerification.Stub.asInterface(
                host.getDomainVerificationBackend());
        DomainVerificationEnforcer enforcer = new DomainVerificationEnforcer(context);
        DomainVerificationProxy proxy = new DomainVerificationProxy() {
            @Override public boolean isCallerVerifier(int uid) {
                try { return host.isDomainVerifier(uid); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
            @Override public ComponentName getComponentName() { throw unsupported(); }
            @Override public void sendBroadcastForPackages(Set<String> names) { throw unsupported(); }
            @Override public boolean runMessage(int code, Object object) { throw unsupported(); }
        };
        DomainVerificationService service = new DomainVerificationService(
                context, SystemConfig.getInstance(), new PlatformCompat(context)) {
            @Override public List<String> queryValidVerificationPackageNames() {
                enforcer.assertApprovedVerifier(caller.get(), proxy);
                try { return backend.queryValidVerificationPackageNames(caller.get()); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
            @Override public DomainVerificationInfo getDomainVerificationInfo(String name) {
                enforcer.assertApprovedQuerent(caller.get(), proxy);
                try { return backend.getDomainVerificationInfo(caller.get(), name); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
            @Override public int setDomainVerificationStatusInternal(
                    int uid, UUID id, Set<String> domains, int state) {
                enforcer.assertApprovedVerifier(uid, proxy);
                try { return backend.setDomainVerificationStatus(uid, id.toString(), new DomainSet(domains), state); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
        };
        service.setConnection(new DomainVerificationManagerInternal.Connection() {
            @Override public int getCallingUid() { return caller.get(); }
            @Override public int getCallingUserId() { throw unsupported(); }
            @Override public int[] getAllUserIds() { throw unsupported(); }
            @Override public Computer snapshot() { throw unsupported(); }
            @Override public void scheduleWriteSettings() { throw unsupported(); }
            @Override public boolean doesUserExist(int user) { throw unsupported(); }
            @Override public boolean filterAppAccess(String name, int uid, int user) { throw unsupported(); }
        });
        entry = new DomainVerificationManagerStub(service);
    }

    private static UnsupportedOperationException unsupported() {
        return new UnsupportedOperationException("domain_verification: outside verifier API (#1763)");
    }

    private void enter(int uid) {
        Bridge.enforceSystemUid();
        caller.set(uid);
    }

    @Override public List<String> queryValidVerificationPackageNames(int uid) {
        enter(uid);
        try { return entry.queryValidVerificationPackageNames(); }
        finally { caller.remove(); }
    }
    @Override public DomainVerificationInfo getDomainVerificationInfo(int uid, String name) {
        enter(uid);
        try { return entry.getDomainVerificationInfo(name); }
        finally { caller.remove(); }
    }
    @Override public int setDomainVerificationStatus(int uid, String id, DomainSet domains, int state) {
        enter(uid);
        try { return entry.setDomainVerificationStatus(id, domains, state); }
        finally { caller.remove(); }
    }
}
