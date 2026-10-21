package com.android.server.adservices;

import android.content.Context;

import com.android.adservices.shared.system.SystemContextSingleton;
import com.android.server.SystemService;
import com.android.server.adservices.data.topics.TopicsDao;

/**
 * AdServicesManagerService's Lifecycle as SystemServer starts it
 * (AD_SERVICES_MANAGER_SERVICE_CLASS, android-16.0.0_r1), for the app host's
 * system process: the service made as `Lifecycle(Context)` makes it and
 * published as `adservices_manager` as its `onStart` publishes it, without
 * that `onStart`'s registration with SdkSandboxManagerLocal, which throws
 * when SdkSandboxManagerService is not started: the app host serves no
 * `sdk_sandbox` (#1982), whose `getAdServicesManager` and dumps are all the
 * registration feeds. In the module's package for the package-private
 * constructors, as SystemServer's class loader holds both.
 */
public final class SystemProcessLifecycle {
    private SystemProcessLifecycle() {}

    public static SystemService start(Context context) {
        AdServicesManagerService service = new AdServicesManagerService(
                SystemContextSingleton.set(context),
                new UserInstanceManager(TopicsDao.getInstance(),
                        AdServicesManagerService.ADSERVICES_BASE_DIR));
        AdServicesManagerService.Lifecycle lifecycle =
                new AdServicesManagerService.Lifecycle(context, service);
        lifecycle.publishBinderService();
        return lifecycle;
    }
}
