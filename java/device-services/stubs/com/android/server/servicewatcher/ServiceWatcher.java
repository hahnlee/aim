// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.servicewatcher;

import android.content.ComponentName;
import android.content.Context;
import android.os.IBinder;
import android.os.RemoteException;

public interface ServiceWatcher {
    interface ServiceListener<TBoundServiceInfo extends BoundServiceInfo> {
        void onBind(IBinder binder, TBoundServiceInfo service) throws RemoteException;
        void onUnbind();
    }

    interface ServiceSupplier<TBoundServiceInfo extends BoundServiceInfo> {
    }

    class BoundServiceInfo {
        protected BoundServiceInfo(String action, int uid, ComponentName componentName) { throw new RuntimeException("stub"); }
        public ComponentName getComponentName() { throw new RuntimeException("stub"); }
    }

    static <TBoundServiceInfo extends BoundServiceInfo> ServiceWatcher create(Context context,
            String tag, ServiceSupplier<TBoundServiceInfo> serviceSupplier,
            ServiceListener<? super TBoundServiceInfo> serviceListener) { throw new RuntimeException("stub"); }

    static <TBoundServiceInfo extends BoundServiceInfo> ServiceWatcher create(Context context,
            String tag, boolean unstableFallbackEnabled,
            ServiceSupplier<TBoundServiceInfo> serviceSupplier,
            ServiceListener<? super TBoundServiceInfo> serviceListener) { throw new RuntimeException("stub"); }

    boolean checkServiceResolves();
    void register();
    void unregister();
}
