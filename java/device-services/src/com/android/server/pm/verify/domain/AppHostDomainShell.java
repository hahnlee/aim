package com.android.server.pm.verify.domain;

import android.content.Context;
import android.content.pm.PackageManager.NameNotFoundException;
import android.content.pm.verify.domain.DomainVerificationUserState;
import android.os.*;
import android.util.*;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.verify.domain.models.*;
import dev.aim.server.IAppHostDomainVerification;
import java.util.*;

/** Original android-16.0.0_r1 shell and debug presentation over one native read capture. */
public final class AppHostDomainShell implements DomainVerificationShell.Callback {
    private final IAppHostDomainVerification backend;
    private final DomainVerificationEnforcer enforcer;
    private final com.android.server.pm.verify.domain.proxy.DomainVerificationProxy proxy;
    public AppHostDomainShell(IAppHostDomainVerification backend, DomainVerificationEnforcer enforcer,
            com.android.server.pm.verify.domain.proxy.DomainVerificationProxy proxy) {
        this.backend = backend; this.enforcer = enforcer; this.proxy = proxy;
    }
    public int run(Context context, boolean packageShell, String[] args,
            ParcelFileDescriptor input, ParcelFileDescriptor output, ParcelFileDescriptor error) {
        var domains = new DomainVerificationShell(this);
        if (packageShell) return com.android.server.pm.NativePackageReadCommand.runDomains(
                context, domains, args, input, output, error);
        return new ShellCommand() {
            @Override public void onHelp() { domains.printHelp(getOutPrintWriter()); }
            @Override public int onCommand(String command) {
                if (command == null) return handleDefaultCommands(command);
                Boolean result = domains.runCommand(this, command);
                return result == null ? handleDefaultCommands(command) : result ? 0 : 1;
            }
        }.exec(new Binder(), input.getFileDescriptor(), output.getFileDescriptor(),
                error.getFileDescriptor(), args, null, null);
    }
    private Parcel read(int kind, String name, List<String> domains, Integer user)
            throws NameNotFoundException {
        enforcer.assertApprovedQuerent(Binder.getCallingUid(), proxy);
        final byte[] bytes;
        try { bytes = backend.readShellState(kind, name, domains, user == null ? 0 : user, user != null); }
        catch (ServiceSpecificException failure) {
            if (failure.errorCode != 1) throw failure;
            throw new NameNotFoundException(name);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        Parcel p = Parcel.obtain(); p.unmarshall(bytes, 0, bytes.length); p.setDataPosition(0);
        return p;
    }
    private static ArraySet<String> set(Parcel p) {
        var result = new ArraySet<String>(); result.addAll(p.createStringArrayList()); return result;
    }
    @Override public void printState(IndentingPrintWriter writer, String name, Integer user)
            throws NameNotFoundException {
        Parcel p = read(0, name, null, user);
        try {
            int size = p.readInt();
            for (int i = 0; i < size; i++) {
                String pkgName = p.readString(); UUID id = UUID.fromString(p.readString());
                AndroidPackage pkg = (AndroidPackage) com.android.server.pm.parsing.PackageCacher
                        .fromCacheEntryStatic(p.createByteArray());
                var valid = set(p); var invalid = set(p); var web = set(p);
                var states = new ArrayMap<String, Integer>();
                int count = p.readInt();
                for (int j = 0; j < count; j++) states.put(p.readString(), p.readInt());
                var users = new SparseArray<DomainVerificationInternalUserState>();
                count = p.readInt();
                for (int j = 0; j < count; j++) {
                    int uid = p.readInt(); boolean allowed = p.readBoolean();
                    users.put(uid, new DomainVerificationInternalUserState(uid, set(p), allowed));
                }
                var state = new DomainVerificationPkgState(pkgName, id, !valid.isEmpty(), states, users, null);
                var debug = new DomainVerificationDebug(new DomainVerificationCollector(null, null) {
                    @Override public ArraySet<String> collectValidAutoVerifyDomains(AndroidPackage ignored) { return valid; }
                    @Override public ArraySet<String> collectInvalidAutoVerifyDomains(AndroidPackage ignored) { return invalid; }
                    @Override public ArraySet<String> collectAllWebDomains(AndroidPackage ignored) { return web; }
                });
                boolean header = debug.printState(writer, state, pkg, new ArrayMap<>(), false);
                debug.printState(writer, state, pkg, user, new ArraySet<>(), name != null || header);
            }
            p.enforceNoDataAvail();
        } finally { p.recycle(); }
    }
    private void owners(IndentingPrintWriter writer, int kind, String name, List<String> domains,
            Integer user) throws NameNotFoundException {
        Parcel p = read(kind, name, domains, user);
        try {
            var debug = new DomainVerificationDebug(null);
            int rows = p.readInt();
            for (int i = 0; i < rows; i++) {
                String header = p.readString(); int count = p.readInt();
                if (header != null && count != 0) { writer.println(header + ":"); writer.increaseIndent(); }
                for (int j = 0; j < count; j++) {
                    String domain = p.readString();
                    var users = new SparseArray<SparseArray<List<String>>>();
                    int size = p.readInt();
                    for (int k = 0; k < size; k++) {
                        int uid = p.readInt(); var levels = new SparseArray<List<String>>();
                        int length = p.readInt();
                        for (int l = 0; l < length; l++) levels.put(p.readInt(), p.createStringArrayList());
                        users.put(uid, levels);
                    }
                    debug.printOwners(writer, domain, users);
                }
                if (header != null && count != 0) writer.decreaseIndent();
            }
            p.enforceNoDataAvail();
        } finally { p.recycle(); }
    }
    @Override public void printOwnersForPackage(IndentingPrintWriter writer, String name, Integer user)
            throws NameNotFoundException { owners(writer, 1, name, null, user); }
    @Override public void printOwnersForDomains(IndentingPrintWriter writer, List<String> domains, Integer user) {
        try { owners(writer, 2, null, domains, user); }
        catch (NameNotFoundException failure) { throw new IllegalStateException(failure); }
    }
    @Override public void setDomainVerificationStatusInternal(String name, int state,
            ArraySet<String> domains) throws NameNotFoundException {
        enforcer.assertInternal(Binder.getCallingUid());
        try { backend.setShellStatus(name, state, domains == null ? null : new ArrayList<>(domains)); }
        catch (ServiceSpecificException failure) {
            if (failure.errorCode != 1) throw failure;
            throw new NameNotFoundException(name);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void setDomainVerificationUserSelectionInternal(int user, String name,
            boolean enabled, ArraySet<String> domains) throws NameNotFoundException {
        enforcer.assertInternal(Binder.getCallingUid());
        try { backend.setShellUserSelection(name, user, enabled,
                domains == null ? null : new ArrayList<>(domains)); }
        catch (ServiceSpecificException failure) {
            if (failure.errorCode != 1) throw failure;
            throw new NameNotFoundException(name);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public DomainVerificationUserState getDomainVerificationUserState(String name, int user) {
        throw new UnsupportedOperationException("unused domain shell user-state callback");
    }
    @Override public void setDomainVerificationLinkHandlingAllowedInternal(String name,
            boolean allowed, int user) throws NameNotFoundException {
        enforcer.assertInternal(Binder.getCallingUid());
        try { backend.setShellLinkHandling(name, user, allowed); }
        catch (ServiceSpecificException failure) {
            if (failure.errorCode != 1) throw failure;
            throw new NameNotFoundException(name);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    private void reset(List<String> names, int user, boolean userOnly) {
        enforcer.assertInternal(Binder.getCallingUid());
        try { backend.resetShellState(names, user, userOnly); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void clearDomainVerificationState(List<String> names) { reset(names, 0, false); }
    @Override public void clearUserStates(List<String> names, int user) { reset(names, user, true); }
    @Override public void verifyPackages(List<String> names, boolean reVerify) {
        enforcer.assertInternal(Binder.getCallingUid());
        try { backend.verifyShellPackages(names, reVerify); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
}
