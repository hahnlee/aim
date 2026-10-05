import android.content.pm.IPackageManager;
import android.os.Process;
import android.os.ServiceManager;

public final class SandboxClientVisibilityOracle {
    public static void main(String[] args) {
        try {
            String name = args[0];
            int clientUid = Integer.parseInt(args[1]);
            if (Process.myUid() != clientUid + 10000) throw new AssertionError("sandbox uid differs");
            var pm = IPackageManager.Stub.asInterface(ServiceManager.getService("package"));
            var app = pm.getApplicationInfo(name, 0, 0);
            if (app == null || app.uid != clientUid) throw new AssertionError("sandbox client application hidden");
            var pkg = pm.getPackageInfo(name, 0, 0);
            if (pkg == null || pkg.applicationInfo.uid != clientUid) throw new AssertionError("sandbox client package hidden");
            if (pm.getApplicationInfo("fixture.missing.sandbox.client", 0, 0) != null)
                throw new AssertionError("missing application exposed");
            System.out.println("SDK_CLIENT " + clientUid + " " + name);
            System.exit(0);
        } catch (Throwable error) { error.printStackTrace(System.out); System.exit(1); }
    }
}
