package dev.darwinart.runtime.am;

import android.os.Binder;
import android.os.IBinder;
import android.os.RemoteException;
import java.util.NoSuchElementException;

/** Controlled resource-port tests; these do not prove genuine BinderProxy JNI behavior. */
public final class ServiceConnectionDeathRegistrationTest {
    private static final class ResourceBinder extends Binder {
        RuntimeException linkFailure;
        NoSuchElementException unlinkFailure;
        int unlinks;
        @Override public void linkToDeath(IBinder.DeathRecipient recipient, int flags)
                throws RemoteException {
            if (linkFailure != null) throw linkFailure;
            super.linkToDeath(recipient, flags);
        }
        @Override public boolean unlinkToDeath(IBinder.DeathRecipient recipient, int flags) {
            unlinks++;
            if (unlinkFailure != null) throw unlinkFailure;
            return super.unlinkToDeath(recipient, flags);
        }
    }

    private static ServiceConnectionDeathRegistration linked(ResourceBinder binder,
            ServiceConnectionDeathRegistration.PresenceQuery query) throws Exception {
        ServiceConnectionDeathRegistration owner =
                new ServiceConnectionDeathRegistration(binder, ignored -> {}, query);
        owner.linkOutsideLock();
        assert !owner.finishLinkLocked();
        assert owner.claimUnlinkLocked();
        return owner;
    }

    private static void typedAbsenceDischargesOnlyAfterPublicUnlink() throws Exception {
        ResourceBinder binder = new ResourceBinder();
        binder.unlinkFailure = new NoSuchElementException("opaque native status");
        int[] queries = {0};
        ServiceConnectionDeathRegistration owner = linked(binder, (target, recipient) -> {
            assert target == binder && binder.unlinks == 1;
            queries[0]++;
            return ServiceConnectionDeathRegistration.ABSENT;
        });
        owner.unlinkOutsideLock();
        assert queries[0] == 1;
    }

    private static void nonAbsenceRetainsOriginalAndAllowsExactRetry() throws Exception {
        for (int outcome : new int[] {ServiceConnectionDeathRegistration.PRESENT,
                ServiceConnectionDeathRegistration.UNSUPPORTED, 77}) {
            ResourceBinder binder = new ResourceBinder();
            NoSuchElementException original = new NoSuchElementException("not absence evidence");
            binder.unlinkFailure = original;
            ServiceConnectionDeathRegistration owner = linked(binder, (target, recipient) -> outcome);
            try {
                owner.unlinkOutsideLock();
                throw new AssertionError("Unproven absence discharged a resource");
            } catch (NoSuchElementException failure) { assert failure == original; }
            binder.unlinkFailure = null;
            owner.unlinkOutsideLock();
            assert binder.unlinks == 2;
        }
    }

    private static void queryFailurePreservesPrimary() throws Exception {
        ResourceBinder binder = new ResourceBinder();
        NoSuchElementException original = new NoSuchElementException("unlink failed");
        Error queryFailure = new LinkageError("query unavailable");
        binder.unlinkFailure = original;
        ServiceConnectionDeathRegistration owner = linked(binder, (target, recipient) -> {
            throw queryFailure;
        });
        try {
            owner.unlinkOutsideLock();
            throw new AssertionError("Query failure discharged a resource");
        } catch (NoSuchElementException failure) {
            assert failure == original;
            assert failure.getSuppressed().length == 1;
            assert failure.getSuppressed()[0] == queryFailure;
        }
    }

    private static void ambiguousReturnedLinkMayProveAbsence() throws Exception {
        ResourceBinder binder = new ResourceBinder();
        RuntimeException original = new IllegalStateException("link invocation failed");
        binder.linkFailure = original;
        binder.unlinkFailure = new NoSuchElementException("never installed");
        ServiceConnectionDeathRegistration owner = new ServiceConnectionDeathRegistration(
                binder, ignored -> {}, (target, recipient) -> ServiceConnectionDeathRegistration.ABSENT);
        try { owner.linkOutsideLock(); throw new AssertionError("Expected link error"); }
        catch (RuntimeException failure) { assert failure == original; }
        assert owner.failLinkLocked(true);
        owner.unlinkOutsideLock();
        assert binder.unlinks == 1;
    }

    private static void uniqueAttemptAndAdmissionAreEnforced() throws Exception {
        ResourceBinder binder = new ResourceBinder();
        ServiceConnectionDeathRegistration owner = new ServiceConnectionDeathRegistration(
                binder, ignored -> {}, (target, recipient) -> {
                    throw new AssertionError("Unstarted link cannot establish absence");
                });
        try { owner.unlinkOutsideLock(); throw new AssertionError("Unstarted unlink accepted"); }
        catch (IllegalStateException expected) { assert binder.unlinks == 0; }
        owner.linkOutsideLock();
        try { owner.linkOutsideLock(); throw new AssertionError("Recipient reused for relink"); }
        catch (IllegalStateException expected) {}
    }

    public static void main(String[] args) throws Exception {
        typedAbsenceDischargesOnlyAfterPublicUnlink();
        nonAbsenceRetainsOriginalAndAllowsExactRetry();
        queryFailurePreservesPrimary();
        ambiguousReturnedLinkMayProveAbsence();
        uniqueAttemptAndAdmissionAreEnforced();
        System.out.println("connection death resources: controlled typed absence/error/retry/unique-attempt PASS");
    }
}
