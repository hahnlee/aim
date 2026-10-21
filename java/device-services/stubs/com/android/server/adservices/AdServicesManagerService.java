// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.adservices;

import android.app.adservices.IAdServicesManager;
import android.content.Context;

import com.android.server.SystemService;

public class AdServicesManagerService extends IAdServicesManager.Stub {
    public static String ADSERVICES_BASE_DIR;

    AdServicesManagerService(Context context, UserInstanceManager userInstanceManager) { throw new RuntimeException("stub"); }

    public static final class Lifecycle extends SystemService {
        public Lifecycle(Context context, AdServicesManagerService service) { super(context); throw new RuntimeException("stub"); }
        @Override public void onStart() { throw new RuntimeException("stub"); }
        void publishBinderService() { throw new RuntimeException("stub"); }
    }
}
