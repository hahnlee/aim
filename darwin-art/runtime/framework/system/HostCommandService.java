package dev.darwinart.runtime.system;

import android.os.IBinder;
import android.os.ParcelFileDescriptor;
import android.os.RemoteException;
import android.os.ResultReceiver;
import android.os.ShellCallback;
import android.util.Slog;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.nio.file.Files;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/**
 * Host commands relayed by the profile daemon (ADR 0009): `cmd SERVICE
 * ARGS...` runs the named service's shell command, as /system/bin/cmd does
 * for adb, and `dumpsys SERVICE ARGS...` its dump, as /system/bin/dumpsys
 * does. `darwin-art install` is `cmd package install`, which
 * PackageManagerShellCommand turns into a PackageInstaller session, and
 * `launcher-info` and `archive-info` are the host launcher's PackageManager
 * queries ({@link LauncherQueries}).
 */
public final class HostCommandService {
    private static final String TAG = "HostCommandService";
    private static final int EXIT_USAGE = 64;
    private static final int EXIT_NO_SERVICE = 69;
    private static final int EXIT_FAILURE = 1;

    private HostCommandService() {}

    private static native long nativeConnect();
    private static native String[] nativeNext(long listener);
    private static native boolean nativeReply(long listener, int status, byte[] output);

    public static void start(ServiceDirectory services) {
        long listener = nativeConnect();
        if (listener == 0) throw new IllegalStateException("host command relay is unavailable");
        Thread thread = new Thread(() -> serve(listener, services), "HostCommands");
        thread.setDaemon(true);
        thread.start();
    }

    private static void serve(long listener, ServiceDirectory services) {
        String[] arguments;
        while ((arguments = nativeNext(listener)) != null) {
            ByteArrayOutputStream output = new ByteArrayOutputStream();
            int status;
            try {
                status = run(services, arguments, output);
            } catch (Exception | Error error) {
                Slog.w(TAG, "host command failed", error);
                output.write(String.valueOf(error).getBytes(), 0, String.valueOf(error).length());
                status = EXIT_FAILURE;
            }
            if (!nativeReply(listener, status, output.toByteArray())) break;
        }
        Slog.w(TAG, "host command relay closed");
    }

    private static int run(ServiceDirectory services, String[] arguments,
            ByteArrayOutputStream output) throws Exception {
        if (arguments.length == 2 && "launcher-info".equals(arguments[0])) {
            File icons = new File(outputDirectory(), "icons");
            if (!icons.isDirectory() && !icons.mkdirs()) {
                throw new IOException("cannot create " + icons);
            }
            return LauncherQueries.launcherInfo(arguments[1], icons, output);
        }
        if (arguments.length == 2 && "archive-info".equals(arguments[0])) {
            return LauncherQueries.archiveInfo(arguments[1], output);
        }
        boolean dump = arguments.length >= 2 && "dumpsys".equals(arguments[0]);
        if (arguments.length < 2 || !(dump || "cmd".equals(arguments[0]))) {
            output.write(("usage: cmd|dumpsys SERVICE [ARGS...] | launcher-info PACKAGE"
                    + " | archive-info APK\n").getBytes());
            return EXIT_USAGE;
        }
        IBinder service = services.localService(arguments[1]);
        if (service == null) {
            output.write((arguments[0] + ": Can't find service: " + arguments[1] + "\n")
                    .getBytes());
            return EXIT_NO_SERVICE;
        }
        String[] serviceArguments = Arrays.copyOfRange(arguments, 2, arguments.length);
        if (dump) return dump(service, serviceArguments, output);
        // The command's stdio: an empty stdin and one output file that
        // interleaves stdout and stderr, read back when the command finishes.
        File directory = outputDirectory();
        File input = new File(directory, "stdin");
        File transcript = new File(directory, "output");
        int mode = ParcelFileDescriptor.MODE_READ_WRITE | ParcelFileDescriptor.MODE_CREATE
                | ParcelFileDescriptor.MODE_TRUNCATE;
        CountDownLatch finished = new CountDownLatch(1);
        int[] result = {EXIT_FAILURE};
        ResultReceiver receiver = new ResultReceiver(null) {
            @Override
            protected void onReceiveResult(int resultCode, android.os.Bundle data) {
                result[0] = resultCode;
                finished.countDown();
            }
        };
        try (ParcelFileDescriptor in = ParcelFileDescriptor.open(input, mode);
                ParcelFileDescriptor out = ParcelFileDescriptor.open(transcript, mode)) {
            service.shellCommand(in.getFileDescriptor(), out.getFileDescriptor(),
                    out.getFileDescriptor(), serviceArguments, new SystemShellCallback(),
                    receiver);
            if (!finished.await(10, TimeUnit.MINUTES)) {
                throw new IllegalStateException("shell command did not finish");
            }
        }
        output.write(Files.readAllBytes(transcript.toPath()));
        return result[0];
    }

    /** dumpsys: the service's Binder.dump into the command's output file. */
    private static int dump(IBinder service, String[] arguments, ByteArrayOutputStream output)
            throws IOException, RemoteException {
        File transcript = new File(outputDirectory(), "dump");
        try (ParcelFileDescriptor out = ParcelFileDescriptor.open(transcript,
                ParcelFileDescriptor.MODE_READ_WRITE | ParcelFileDescriptor.MODE_CREATE
                        | ParcelFileDescriptor.MODE_TRUNCATE)) {
            service.dump(out.getFileDescriptor(), arguments);
        }
        output.write(Files.readAllBytes(transcript.toPath()));
        return 0;
    }

    private static File outputDirectory() throws IOException {
        File directory = new File("/data/system/host-command");
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IOException("cannot create " + directory);
        }
        return directory;
    }

    /**
     * The shell's file access for commands that name files (`install PATH`):
     * the command runs on behalf of the host user, whose files the system
     * server reads under its own /data (adb's /data/local/tmp).
     */
    private static final class SystemShellCallback extends ShellCallback {
        @Override
        public ParcelFileDescriptor onOpenFile(String path, String seLinuxContext, String mode) {
            if (!"r".equals(mode)) throw new SecurityException("host commands open files read-only");
            try {
                return ParcelFileDescriptor.open(new File(path), ParcelFileDescriptor.MODE_READ_ONLY);
            } catch (FileNotFoundException error) {
                return null;
            }
        }
    }
}
