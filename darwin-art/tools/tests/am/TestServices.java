package dev.darwinart.runtime.am;

import android.content.ComponentName;
import android.content.pm.ApplicationInfo;
import android.content.pm.ServiceInfo;
import dev.darwinart.runtime.pm.ServiceResolver;

/** PackageManager service declarations for one installed test package. */
final class TestServices {
    private TestServices() {}

    /** Resolves each {@code className, processName} pair as a non-exported service. */
    static ServiceResolver resolver(String packageName, int uid, String... declarations) {
        if ((declarations.length & 1) != 0) throw new IllegalArgumentException("pairs");
        return component -> {
            if (component == null || !packageName.equals(component.getPackageName())) {
                return null;
            }
            for (int index = 0; index < declarations.length; index += 2) {
                if (declarations[index].equals(component.getClassName())) {
                    return service(packageName, uid, declarations[index],
                            declarations[index + 1]);
                }
            }
            return null;
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
