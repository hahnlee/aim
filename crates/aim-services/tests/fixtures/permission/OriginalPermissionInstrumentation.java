package dev.aim.test.permission;

import android.app.ActivityManager;
import android.app.Instrumentation;
import android.app.UiAutomation;
import android.os.Bundle;
import android.os.Process;

/** Exercises the original permission authority and real instrumentation delegation. */
public final class OriginalPermissionInstrumentation extends Instrumentation {
    private static final String PERMISSION = "android.permission.READ_CONTACTS";
    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }
    private static int check(String name, int pid, int uid) throws Exception {
        var activity = ActivityManager.getService();
        if (activity == null) throw new IllegalStateException("original activity owner missing");
        int result = activity.checkPermission(name, pid, uid);
        if (result != 0 && result != -1) throw new IllegalStateException("invalid permission result " + result);
        return result;
    }
    private static void expect(int actual, int expected, String stage) {
        if (actual != expected) throw new AssertionError(stage + ": " + actual + " expected " + expected);
    }
    @Override public void onStart() {
        Bundle result = new Bundle();
        UiAutomation automation = null;
        try {
            int pid = Process.myPid(), uid = Process.myUid();
            expect(check("android.permission.INTERACT_ACROSS_USERS_FULL", -1, Process.SYSTEM_UID), 0, "system allowed");
            expect(check(PERMISSION, pid, uid), -1, "caller denied before delegation");
            automation = getUiAutomation();
            if (automation == null) throw new IllegalStateException("original UiAutomation owner missing");
            automation.adoptShellPermissionIdentity(PERMISSION);
            expect(check(PERMISSION, pid, uid), 0, "actual instrumentation PID delegated");
            automation.dropShellPermissionIdentity();
            expect(check(PERMISSION, pid, uid), -1, "denied after delegation drop");
            result.putString("permission_proof", "allowed,denied,delegated,dropped pid=" + pid + " uid=" + uid);
            finish(-1, result);
        } catch (Throwable failure) {
            if (automation != null) {
                try { automation.dropShellPermissionIdentity(); }
                catch (Throwable cleanup) { failure.addSuppressed(cleanup); }
            }
            java.io.StringWriter text = new java.io.StringWriter();
            failure.printStackTrace(new java.io.PrintWriter(text));
            result.putString("permission_failure", text.toString());
            finish(0, result);
        }
    }
}
