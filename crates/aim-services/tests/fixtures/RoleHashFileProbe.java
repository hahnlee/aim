package dev.aim.test;
import android.os.Process;
import android.os.ServiceManager;
import dev.aim.server.IServiceHost;
import java.io.File;
import java.io.FileOutputStream;

/** Read-only owner diagnostic; output is a fresh regular file in disposable data. */
public final class RoleHashFileProbe {
    private RoleHashFileProbe() {}
    public static void main(String[] args) throws Exception {
        if (Process.myUid() != 1000) throw new SecurityException("Role diagnostic requires uid1000");
        if (args.length != 1) throw new IllegalArgumentException("USER required");
        int user = Integer.parseInt(args[0]);
        if (user < 0) throw new IllegalArgumentException("USER must be nonnegative");
        var binder = ServiceManager.getService("aim.service_host");
        if (binder == null) throw new IllegalStateException("Native service host unavailable");
        var host = IServiceHost.Stub.asInterface(binder);
        File directory = new File("/data/local/tmp/aim-role-hash");
        if (!directory.isDirectory()) throw new IllegalStateException("Owned diagnostic directory not staged");
        File output = File.createTempFile("rolehash-", ".txt", directory);
        try (FileOutputStream stream = new FileOutputStream(output)) {
            host.asBinder().dump(stream.getFD(), new String[] {"--role-package-state-hash", Integer.toString(user)});
            stream.getFD().sync();
        }
        System.out.println("ROLE_HASH_OUTPUT\t" + output.getAbsolutePath());
    }
}
