package dev.aim.server;
import android.os.CreateAppDataArgs;
import android.os.CreateAppDataResult;
import com.android.server.pm.Installer;
import java.util.ArrayList;
import java.util.Arrays;

final class ExistingInstallJavaTest {
    static final class Owner extends Installer {
        final ArrayList<String> calls = new ArrayList<>();
        int failure;
        Owner() { super(null); }
        CreateAppDataResult result() {
            var r = new CreateAppDataResult(); r.ceDataInode = 71; r.deDataInode = -1; return r;
        }
        @Override public CreateAppDataResult[] createAppDataBatched(CreateAppDataArgs[] args) throws InstallerException {
            if (!Thread.holdsLock(this) || args.length != 1 || args[0].flags != 11
                    || args[0].previousAppId != 0 || !args[0].seInfo.equals("default:complete")) throw new AssertionError();
            calls.add("batch");
            if (failure == 1) throw new InstallerException("batch failed");
            var r = result();
            if (failure >= 2) { r.exceptionCode = 5; r.exceptionMessage = "create failed"; }
            return new CreateAppDataResult[] {r};
        }
        @Override public CreateAppDataResult createAppData(CreateAppDataArgs args) throws InstallerException {
            calls.add("retry");
            if (failure == 3) throw new InstallerException("retry failed");
            return result();
        }
        @Override public void destroyAppData(String uuid, String name, int user, int flags, long inode) {
            if (uuid != null || !name.equals("p") || user != 0 || flags != 3 || inode != 17) throw new AssertionError();
            calls.add("destroy");
        }
        @Override public void linkNativeLibraryDirectory(String uuid, String name, String path, int user) {
            if (!Thread.holdsLock(this)) throw new AssertionError();
            calls.add("link");
        }
    }
    static void run(String root) throws Exception {
        var args = new CreateAppDataArgs(); args.packageName = "p"; args.flags = 11;
        args.seInfo = "default:complete";
        for (int failure : new int[] {0, 1, 2, 3}) {
            var owner = new Owner(); owner.failure = failure;
            try {
                var result = ExistingInstallEffects.prepareAppData(owner, args, 3, 17,
                        "armeabi-v7a", root, () -> owner.calls.add("external"));
                if (failure == 3 || !Arrays.equals(result, new long[] {3, 71, -1})) throw new AssertionError();
                var expected = failure == 0 ? Arrays.asList("batch", "link", "external")
                        : Arrays.asList("batch", "destroy", "retry", "link", "external");
                if (!owner.calls.equals(expected)) throw new AssertionError(owner.calls);
            } catch (Installer.InstallerException e) {
                if (failure != 3 || !e.getMessage().equals("retry failed")
                        || !owner.calls.equals(Arrays.asList("batch", "destroy", "retry"))) throw new AssertionError(e);
            }
        }
        for (String abi : new String[] {null, "arm64-v8a", "armeabi-v7a"}) {
            var owner = new Owner();
            ExistingInstallEffects.prepareAppData(owner, args, 1, 17, abi, root, () -> {});
            if (!owner.calls.equals(Arrays.asList("batch"))) throw new AssertionError(owner.calls);
        }
        var owner = new Owner();
        ExistingInstallEffects.prepareAppData(owner, args, 3, 17, "armeabi-v7a", root + "/absent", () -> {});
        if (!owner.calls.equals(Arrays.asList("batch"))) throw new AssertionError(owner.calls);
    }
}
