package dev.darwinart.runtime.am;

import android.content.ComponentName;
import android.content.pm.ApplicationInfo;
import android.content.pm.ServiceInfo;

/** PackageManager service declarations for one installed test package. */
final class TestServices {
    private TestServices() {}

    /** Isolated uids registered with the package manager, by owner. */
    static final java.util.Map<Integer, Integer> ISOLATED_OWNERS = new java.util.HashMap<>();

    /** Resolves each {@code className, processName} pair as a non-exported service. */
    static PackageQueries resolver(String packageName, int uid, String... declarations) {
        if ((declarations.length & 1) != 0) throw new IllegalArgumentException("pairs");
        return new PackageQueries() {
            @Override
            public ServiceInfo service(ComponentName component, int userId) {
                if (component == null || !packageName.equals(component.getPackageName())) {
                    return null;
                }
                for (int index = 0; index < declarations.length; index += 2) {
                    if (declarations[index].equals(component.getClassName())) {
                        return TestServices.service(packageName, uid, declarations[index],
                                declarations[index + 1]);
                    }
                }
                return null;
            }

            @Override
            public void addIsolatedUid(int isolatedUid, int ownerUid) {
                if (ownerUid != uid || ISOLATED_OWNERS.put(isolatedUid, ownerUid) != null) {
                    throw new AssertionError("isolated uid " + isolatedUid + " registered twice");
                }
            }

            @Override
            public void removeIsolatedUid(int isolatedUid) {
                if (ISOLATED_OWNERS.remove(isolatedUid) == null) {
                    throw new AssertionError("isolated uid " + isolatedUid + " was not registered");
                }
            }
        };
    }

    static ServiceInfo service(String packageName, int uid, String className,
            String processName) {
        ApplicationInfo application = new ApplicationInfo();
        application.packageName = packageName;
        application.processName = packageName;
        application.uid = uid;
        application.enabled = true;
        ServiceInfo info = new ServiceInfo();
        info.packageName = packageName;
        info.name = className;
        info.processName = processName;
        info.applicationInfo = application;
        info.enabled = true;
        return info;
    }
}
