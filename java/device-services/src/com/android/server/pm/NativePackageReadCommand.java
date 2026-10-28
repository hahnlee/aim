package com.android.server.pm;

import android.content.Context;
import android.content.pm.IPackageManager;
import android.os.ParcelFileDescriptor;
import android.os.Binder;
import android.os.ShellCommand;
import android.os.IBinder;
import java.io.PrintWriter;

/** Cold commands use the pinned shell, with caller-bound native package reads. */
public final class NativePackageReadCommand {
    private NativePackageReadCommand() {}
    public static int run(Context context, IPackageManager packages, IBinder[] dumpServices, String[] args,
            ParcelFileDescriptor input, ParcelFileDescriptor output, ParcelFileDescriptor error) {
        try (input; output; error) {
            return execute(context, packages, dumpServices, args, input, output, error);
        } catch (java.io.IOException failure) { throw new IllegalStateException(failure); }
    }
    public static int runDomains(Context context,
            com.android.server.pm.verify.domain.DomainVerificationShell domains, String[] args,
            ParcelFileDescriptor input, ParcelFileDescriptor output, ParcelFileDescriptor error) {
        return new PackageManagerShellCommand(null, context, domains).exec(new Binder(),
                input.getFileDescriptor(), output.getFileDescriptor(), error.getFileDescriptor(),
                args, null, null);
    }
    private static int execute(Context context, IPackageManager packages, IBinder[] dumpServices, String[] args,
            ParcelFileDescriptor input, ParcelFileDescriptor output, ParcelFileDescriptor error) {
        if (args == null || args.length == 0) throw new IllegalArgumentException("No read command");
        switch (args[0]) {
            case "dump":
            case "dump-package":
            case "get-privapp-permissions":
            case "get-privapp-deny-permissions":
            case "get-oem-permissions":
            case "get-signature-permission-allowlist":
            case "get-shared-uid-allowlist":
                break;
            default: throw new IllegalArgumentException("Unknown independent read command");
        }
        // These commands never enter the domain shell or the PMS local owner.
        ShellCommand command = args[0].startsWith("dump") ? new ShellCommand() {
            @Override public void onHelp() {}
            @Override public int onCommand(String ignored) {
                String pkg = getNextArg();
                if (pkg == null) {
                    getErrPrintWriter().println("Error: no package specified");
                    return 1;
                }
                // runDumpPackage uses the local PMS IBinder cast. The scoped
                // native owner is a proxy, so take its Binder explicitly.
                if ("dump".equals(args[0])) {
                    dumpPackageState(getOutPrintWriter(), output.getFileDescriptor(), pkg, dumpServices);
                    return 0;
                }
                try { packages.asBinder().dump(output.getFileDescriptor(), new String[]{pkg}); }
                catch (Throwable failure) {
                    PrintWriter pw = getErrPrintWriter();
                    pw.println("Failure dumping service:");
                    failure.printStackTrace(pw);
                    pw.flush();
                }
                return 0;
            }
        } : new PackageManagerShellCommand(packages, context, null);
        return command.exec(new Binder(),
                input.getFileDescriptor(), output.getFileDescriptor(), error.getFileDescriptor(),
                args, null, null);
    }

    // ActivityManager.dumpPackageStateStatic/dumpService, android-16.0.0_r1,
    // AOSP Apache-2.0. Native owners were local to system_server: execute their
    // scoped capabilities synchronously, with the actual caller on every dump.
    private static void dumpPackageState(PrintWriter pw, java.io.FileDescriptor fd, String pkg,
            IBinder[] services) {
        if (services == null || services.length != 6)
            throw new IllegalStateException("Package dump service capture unavailable (#2327)");
        String[] names = {"package", "activity", "meminfo", "procstats", "usagestats", "batterystats"};
        String[][] arguments = {{pkg}, {"-a", "package", pkg}, {"--local", "--package", pkg},
                {pkg}, {pkg}, {pkg}};
        for (int i = 0; i < names.length; i++) {
            if (i != 0) pw.println();
            pw.println("DUMP OF SERVICE " + names[i] + ":");
            pw.flush();
            if (services[i] == null) {
                pw.println("  (Service not found)");
            } else {
                try { services[i].dump(fd, arguments[i]); }
                catch (Throwable failure) {
                    pw.println("Failure dumping service:");
                    failure.printStackTrace(pw);
                }
            }
            pw.flush();
        }
    }
}
