import android.os.IInstalld;
import android.os.ReconcileSdkDataArgs;
import android.os.ServiceManager;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

public final class SdkDataOracle {
    private static final String PACKAGE = "fixture.native.sdk.client";
    private static final String ORIGINAL = "fixture.original.sdk.client";
    public static void main(String[] args) {
        try {
            var owner = dev.aim.server.PackageBootstrapBridge.sdkDataOwner();
            if (args.length > 1) {
                if (android.os.Process.myUid() != 19001) throw new AssertionError("foreign SDK caller differs");
                try { owner.reconcile(null, PACKAGE, List.of("sdk-b"), 0, 19001, 0, "default", 3);
                    throw new AssertionError("foreign SDK caller accepted"); }
                catch (java.io.IOException expected) {
                    if (!expected.getMessage().startsWith("java.lang.SecurityException:")) throw expected;
                }
                System.out.println("SDK_DENIED"); System.exit(0);
            }
            var installd = IInstalld.Stub.asInterface(ServiceManager.getService("installd"));
            var request = new ReconcileSdkDataArgs();
            request.uuid = null; request.packageName = PACKAGE;
            request.subDirNames = List.of("sdk-a", "sdk-b"); request.userId = 0;
            request.appId = 19001; request.previousAppId = 0; request.seInfo = "default"; request.flags = 1;
            var parcel = android.os.Parcel.obtain();
            try {
                request.writeToParcel(parcel, 0);
                Files.write(Path.of(args[0], "sdk-args.original"), parcel.marshall());
            } finally { parcel.recycle(); }
            dev.aim.server.PackageLocal.SdkDataOwner original = (volume, name, dirs, user, app, previous, seinfo, flags) -> {
                var direct = new ReconcileSdkDataArgs();
                direct.uuid = volume; direct.packageName = name; direct.subDirNames = dirs;
                direct.userId = user; direct.appId = app; direct.previousAppId = previous;
                direct.seInfo = seinfo; direct.flags = flags;
                try { installd.reconcileSdkData(direct); }
                catch (Exception failure) { throw new java.io.IOException(failure.toString()); }
            };
            String originalMigration = reconcile(original, ORIGINAL);
            String nativeMigration = reconcile(owner, PACKAGE);
            if (!originalMigration.replace(ORIGINAL, PACKAGE).equals(nativeMigration))
                throw new AssertionError("SDK migration failure differs: " + nativeMigration);
            request.userId = -1;
            String failure;
            try { installd.reconcileSdkData(request); throw new AssertionError("original invalid user accepted"); }
            catch (android.os.ServiceSpecificException expected) { failure = expected.getMessage(); }
            try { owner.reconcile(null, PACKAGE, request.subDirNames, -1, 19001, 0, "default", 1);
                throw new AssertionError("native invalid user accepted"); }
            catch (java.io.IOException expected) {
                // ENFORCE_VALID_USER calls error(message), which returns the Binder worker's
                // current errno. Independent requests can have different numeric codes.
                String prefix = "android.os.ServiceSpecificException: " + failure + " (code ";
                String message = expected.getMessage();
                if (!message.startsWith(prefix) || !message.endsWith(")")
                        || Integer.parseInt(message.substring(prefix.length(), message.length() - 1)) < 0)
                    throw new AssertionError("SDK failure type/message differs: " + expected);
            }
            System.out.println("SDK_DATA"); System.exit(0);
        } catch (Throwable error) { error.printStackTrace(System.out); System.exit(1); }
    }
    private static String reconcile(dev.aim.server.PackageLocal.SdkDataOwner owner, String name) throws Exception {
        owner.reconcile(null, name, List.of("sdk-a", "sdk-b"), 0, 19001, 0, "default", 1);
        check(name, "misc_de", "sdk-a", true); check(name, "misc_de", "sdk-b", true);
        check(name, "misc_ce", "sdk-b", false);
        owner.reconcile(null, name, List.of("sdk-b"), 0, 19001, 0, "default", 2);
        check(name, "misc_ce", "sdk-b", true); check(name, "misc_de", "sdk-a", true);
        owner.reconcile(null, name, List.of("sdk-b"), 0, 19001, 19001, "default", 3);
        check(name, "misc_de", "sdk-a", false); check(name, "misc_de", "sdk-b", true);
        check(name, "misc_ce", "sdk-b", true);
        // The pinned original chowns the SDK root's group to the new UID,
        // then fs_prepare_dir_strict rejects it because SDK roots require AID_NOBODY.
        try { owner.reconcile(null, name, List.of("sdk-b"), 0, 19002, 19001, "default", 3);
            throw new AssertionError("SDK UID migration unexpectedly succeeded"); }
        catch (java.io.IOException expected) {
            if (!expected.getMessage().startsWith("android.os.ServiceSpecificException: Failed to prepare ")) throw expected;
            return expected.getMessage();
        }
    }
    private static void check(String name, String storage, String sdk, boolean exists) {
        Path path = Path.of("/data", storage, "0/sdksandbox", name, sdk);
        if (Files.isDirectory(path) != exists) throw new AssertionError("SDK path differs: " + path);
        if (exists && (!Files.isDirectory(path.resolve("cache")) || !Files.isDirectory(path.resolve("code_cache"))))
            throw new AssertionError("SDK caches missing: " + path);
    }
}
