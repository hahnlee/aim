package dev.aim.server;

import com.android.server.LocalServices;
import com.android.server.wm.ActivityInterceptorCallback;
import com.android.server.wm.ActivityTaskManagerInternal;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * The device's activity start interceptor: WindowManager takes one per id,
 * and a product has one id (PRODUCT_ORDERED_ID). Each start goes to every
 * callback added, in order, and the first result wins.
 */
final class ProductInterceptor implements ActivityInterceptorCallback {
    private static ProductInterceptor sInstance;

    private final List<ActivityInterceptorCallback> mCallbacks = new CopyOnWriteArrayList<>();

    private ProductInterceptor() {}

    /** Adds `callback`; one of each kind, as WindowManager allows one per id. */
    static synchronized void add(ActivityInterceptorCallback callback) {
        if (sInstance == null) {
            sInstance = new ProductInterceptor();
            LocalServices.getService(ActivityTaskManagerInternal.class)
                    .registerActivityStartInterceptor(PRODUCT_ORDERED_ID, sInstance);
        }
        for (ActivityInterceptorCallback c : sInstance.mCallbacks) {
            if (c.getClass() == callback.getClass()) {
                throw new IllegalArgumentException("Duplicate interceptor: " + callback);
            }
        }
        sInstance.mCallbacks.add(callback);
    }

    @Override
    public ActivityInterceptResult onInterceptActivityLaunch(ActivityInterceptorInfo info) {
        for (ActivityInterceptorCallback callback : mCallbacks) {
            ActivityInterceptResult result = callback.onInterceptActivityLaunch(info);
            if (result != null) {
                return result;
            }
        }
        return null;
    }
}
