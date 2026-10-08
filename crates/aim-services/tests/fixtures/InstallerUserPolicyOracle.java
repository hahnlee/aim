package dev.aim.server;

import android.app.admin.DevicePolicyManagerInternal;
import android.os.Parcel;
import android.os.UserManager;
import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;
import java.nio.file.Files;
import java.nio.file.Path;

/** Original Parcel/LocalServices ABI with explicit test owners, not live SystemServer policy. */
public final class InstallerUserPolicyOracle {
    private static final int USER = 43;

    private static final class FixtureUsers extends UserManagerInternal {
        int flags;
        RuntimeException failure;
        Runnable replace;
        public void addUserVisibilityListener(UserVisibilityListener listener) { throw new AssertionError("unused visibility owner"); }
        public int[] getUserIds() { throw new AssertionError("unused user inventory owner"); }
        public boolean exists(int user) {
            if (user != USER) throw new AssertionError("user identity changed");
            return (flags & 1) != 0;
        }
        public boolean hasUserRestriction(String restriction, int user) {
            if (failure != null) throw failure;
            if (user != USER) throw new AssertionError("restriction user changed");
            if (replace != null) { Runnable action = replace; replace = null; action.run(); }
            if (restriction.equals(UserManager.DISALLOW_INSTALL_APPS)) return (flags & 2) != 0;
            if (restriction.equals(UserManager.DISALLOW_DEBUGGING_FEATURES)) return (flags & 4) != 0;
            throw new AssertionError("unknown restriction queried");
        }
    }
    private static final class FixtureDevicePolicy extends DevicePolicyManagerInternal {
        boolean managed;
        Runnable replace;
        public boolean isUserOrganizationManaged(int user) {
            if (user != USER) throw new AssertionError("managed user changed");
            if (replace != null) { Runnable action = replace; replace = null; action.run(); }
            return managed;
        }
    }
    private static void clear() {
        LocalServices.removeServiceForTest(UserManagerInternal.class);
        LocalServices.removeServiceForTest(DevicePolicyManagerInternal.class);
    }
    private static void missing() {
        try { InstallerUserPolicy.capture(USER); throw new AssertionError("missing policy owner accepted"); }
        catch (IllegalStateException expected) {}
    }
    public static void main(String[] args) {
        try {
            clear();
            boolean revocable = android.content.pm.PackageInstaller.ENABLE_REVOCABLE_FD;
            Files.write(Path.of(args[0], "installer-revocable-fd.original"), new byte[] { (byte)(revocable ? 1 : 0) });
            System.out.println("original installer ENABLE_REVOCABLE_FD=" + revocable);
            try { InstallerUserPolicy.capture(-1); throw new AssertionError("negative user accepted"); }
            catch (IllegalArgumentException expected) {}
            missing();
            try { InstallerUserPolicy.shellDebuggingRestricted(USER); throw new AssertionError("missing shell policy owner accepted"); }
            catch (IllegalStateException expected) {}
            var users = new FixtureUsers();
            LocalServices.addService(UserManagerInternal.class, users);
            for (int flags : new int[] { 0, 4 }) {
                users.flags = flags;
                if (InstallerUserPolicy.shellDebuggingRestricted(USER) != (flags == 4))
                    throw new AssertionError("shell restriction changed without DPM or existing user");
            }
            users.flags = 0;
            missing();
            LocalServices.removeServiceForTest(UserManagerInternal.class);
            var policy = new FixtureDevicePolicy();
            LocalServices.addService(DevicePolicyManagerInternal.class, policy);
            missing();
            LocalServices.addService(UserManagerInternal.class, users);
            for (int flags = 0; flags < 16; flags++) {
                users.flags = flags;
                policy.managed = (flags & 8) != 0;
                byte[] bytes = InstallerUserPolicy.capture(USER);
                var in = Parcel.obtain();
                try {
                    in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
                    if (in.readInt() != USER) throw new AssertionError("record user changed");
                    for (int bit = 0; bit < 4; bit++) {
                        if (in.readInt() != ((flags >>> bit) & 1))
                            throw new AssertionError("policy boolean field changed: " + bit);
                    }
                    if (in.dataAvail() != 0) throw new AssertionError("policy record has tail");
                } finally { in.recycle(); }
                Files.write(Path.of(args[0], "policy-" + flags + ".original"), bytes);
            }
            var failure = new IllegalArgumentException("fixture restriction owner failure");
            users.failure = failure;
            try { InstallerUserPolicy.capture(USER); throw new AssertionError("owner failure swallowed"); }
            catch (IllegalArgumentException expected) { if (expected != failure) throw expected; }
            try { InstallerUserPolicy.shellDebuggingRestricted(USER); throw new AssertionError("shell owner failure swallowed"); }
            catch (IllegalArgumentException expected) { if (expected != failure) throw expected; }
            users.failure = null;
            users.replace = () -> {
                LocalServices.removeServiceForTest(UserManagerInternal.class);
                LocalServices.addService(UserManagerInternal.class, new FixtureUsers());
            };
            try { InstallerUserPolicy.shellDebuggingRestricted(USER); throw new AssertionError("replaced shell policy owner accepted"); }
            catch (IllegalStateException expected) {}
            LocalServices.removeServiceForTest(UserManagerInternal.class);
            LocalServices.addService(UserManagerInternal.class, users);
            users.replace = () -> {
                LocalServices.removeServiceForTest(UserManagerInternal.class);
                LocalServices.addService(UserManagerInternal.class, new FixtureUsers());
            };
            missing();
            LocalServices.removeServiceForTest(UserManagerInternal.class);
            LocalServices.addService(UserManagerInternal.class, users);
            policy.replace = () -> {
                LocalServices.removeServiceForTest(DevicePolicyManagerInternal.class);
                LocalServices.addService(DevicePolicyManagerInternal.class, new FixtureDevicePolicy());
            };
            missing();
            byte[] payload = new byte[17003];
            for (int i = 0; i < payload.length; i++) payload[i] = (byte)(i * 31 + 7);
            try (var output = new android.os.FileBridge.FileBridgeOutputStream(android.os.ParcelFileDescriptor.fromFd(0))) {
                output.write(payload, 0, 73);
                output.write(payload, 73, payload.length - 73);
                output.fsync();
            }
            System.out.println("original FileBridgeOutputStream write/fsync/close checks passed");
            System.out.println("original installer user-policy record/test-owner checks passed (not live SystemServer)");
            System.exit(0);
        } catch (Throwable failure) {
            failure.printStackTrace(System.out); System.exit(1);
        } finally { clear(); }
    }
}
