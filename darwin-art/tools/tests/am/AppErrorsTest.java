package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import android.app.ApplicationErrorReport;
import android.os.IBinder;
import java.util.List;

/** AppErrors: crash records, per-uid visibility and removal with the process. */
public final class AppErrorsTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static IBinder attach(ApplicationProcessRegistry registry, int pid, int uid,
            String packageName) {
        IBinder thread = (IBinder) java.lang.reflect.Proxy.newProxyInstance(
                AppErrorsTest.class.getClassLoader(), new Class<?>[] {IBinder.class},
                (proxy, method, arguments) -> {
                    switch (method.getName()) {
                        case "hashCode": return System.identityHashCode(proxy);
                        case "equals": return proxy == arguments[0];
                        case "isBinderAlive": case "pingBinder": return true;
                        default: return null;
                    }
                });
        registry.beginAttachment(pid, uid, thread, 0);
        registry.identify(pid, 0, thread, packageName);
        registry.finishAttachment(pid, 0);
        return thread;
    }

    public static void main(String[] args) {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        IBinder crashedThread = attach(registry, 41, 10000, "org.example.crash");
        attach(registry, 43, 10001, "org.example.other");
        AppErrors errors = new AppErrors(registry);
        check(errors.errorStates(10000) == null, "no crash was reported as an error state");

        ApplicationErrorReport.ParcelableCrashInfo crash =
                new ApplicationErrorReport.ParcelableCrashInfo();
        crash.exceptionClassName = "java.lang.NullPointerException";
        crash.exceptionMessage = "boom";
        crash.stackTrace = "at Example.run(Example.java:1)";
        errors.crashApplication(41, 10000, "org.example.crash", crash);

        List<ActivityManager.ProcessErrorStateInfo> own = errors.errorStates(10000);
        check(own != null && own.size() == 1, "the crashed uid does not see its crash");
        ActivityManager.ProcessErrorStateInfo info = own.get(0);
        check(info.condition == ActivityManager.ProcessErrorStateInfo.CRASHED
                && info.pid == 41 && "org.example.crash".equals(info.processName)
                && "java.lang.NullPointerException".equals(info.shortMsg)
                && info.longMsg.endsWith(": boom") && info.stackTrace.contains("Example"),
                "crash record fields");
        check(errors.errorStates(10001) == null, "another uid saw the crash");
        check(errors.errorStates(1000) != null, "the system did not see the crash");

        registry.retireAttached(41, 0, crashedThread);
        check(errors.errorStates(10000) == null && errors.errorStates(1000) == null,
                "a dead process stayed in the error state");
        System.out.println("app-errors: crash record, visibility and death PASS");
    }
}
