package dev.darwinart.runtime.am;

import android.os.IBinder;
import dev.darwinart.runtime.os.SystemServices;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;

/** Calls the original AIDL proxy; never builds application state in the client. */
public final class ActivityManagerClient {
    private static Object service;
    private ActivityManagerClient() {}
    private static synchronized Object service() throws Exception {
        if (service == null) {
            IBinder binder = SystemServices.getService("activity");
            if (binder == null) throw new IllegalStateException("ActivityManager not published");
            service = Class.forName("android.app.IActivityManager$Stub")
                    .getMethod("asInterface", IBinder.class).invoke(null, binder);
        }
        return service;
    }
    public static Object invoke(Method method, Object[] args) throws Exception {
        try {
            return method.invoke(service(), args);
        } catch (InvocationTargetException error) {
            Throwable cause = error.getCause();
            if (cause instanceof Exception) throw (Exception) cause;
            if (cause instanceof Error) throw (Error) cause;
            throw new IllegalStateException(cause);
        }
    }
}
