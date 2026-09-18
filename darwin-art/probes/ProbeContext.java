package dev.darwinart.probe;

import android.content.AttributionSource;
import android.content.BroadcastReceiver;
import android.content.ContentResolver;
import android.content.ContentCaptureOptions;
import android.content.ComponentCallbacks;
import android.content.Context;
import android.content.ContextWrapper;
import android.content.ComponentName;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.content.pm.ApplicationInfo;
import dev.darwinart.runtime.os.RemoteBinder;
import android.content.pm.PackageManager;
import android.content.pm.ProbeShortcutManager;
import android.content.pm.ShortcutManager;
import android.content.res.Resources;
import android.content.res.Configuration;
import android.os.Looper;
import android.os.Handler;
import android.os.Bundle;
import android.os.Binder;
import android.os.IBinder;
import android.os.Process;
import android.os.Parcel;
import android.os.ProbeUserManager;
import android.os.RemoteException;
import android.os.UserHandle;
import android.os.UserManager;
import android.media.ProbeAudioManager;
import android.view.autofill.AutofillManager;
import android.view.Display;

import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileNotFoundException;
import java.io.FileOutputStream;
import java.util.HashMap;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executor;
import java.util.concurrent.Executors;
import android.app.Application;
import android.app.Service;
import android.app.job.JobScheduler;
import android.test.mock.MockContext;

public final class ProbeContext extends ContextWrapper {
    // Waiting for a newly spawned ART process must not block the caller's
    // Looper. In particular Chromium needs that Looper to connect its GPU.
    private static final Executor serviceReadiness = Executors.newFixedThreadPool(4);
    /**
     * Terminal ContextImpl-shaped owner for APIs that deliberately unwrap every
     * ContextWrapper before invoking hidden framework methods. Android's real
     * Application chain ends at ContextImpl; ending at null made Chromium's
     * bindServiceAsUser path fail before ActivityManager saw the request.
     */
    private static final class BaseContext extends MockContext {
        private ProbeContext owner;

        void attach(ProbeContext context) {
            if (owner != null) throw new IllegalStateException("base Context already attached");
            owner = context;
        }

        private ProbeContext owner() {
            if (owner == null) throw new IllegalStateException("base Context not attached");
            return owner;
        }

        @Override
        public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return owner().bindService(intent, connection, flags);
        }

        @Override
        public boolean bindServiceAsUser(Intent intent, ServiceConnection connection, int flags,
                UserHandle user) {
            return owner().bindServiceAsUser(intent, connection, flags, user);
        }

        public boolean bindServiceAsUser(Intent intent, ServiceConnection connection, int flags,
                Handler handler, UserHandle user) {
            return owner().bindServiceAsUser(intent, connection, flags, handler, user);
        }

        @Override
        public void unbindService(ServiceConnection connection) {
            owner().unbindService(connection);
        }

        @Override
        public boolean isUiContext() {
            // This ContextImpl-shaped endpoint backs the Activity window, not
            // an application-only context. Chromium checks this after
            // deliberately unwrapping ContextWrappers during FRE creation.
            return true;
        }

        @Override
        public void sendBroadcast(Intent intent) {
            owner().sendBroadcast(intent);
        }

        @Override
        public void sendBroadcast(Intent intent, String receiverPermission) {
            owner().sendBroadcast(intent, receiverPermission);
        }

        @Override
        public void sendBroadcastAsUser(Intent intent, UserHandle user) {
            owner().sendBroadcastAsUser(intent, user);
        }
    }

    private final ApplicationInfo applicationInfo;
    private final AttributionSource attributionSource;
    private final ContentResolver contentResolver;
    private final Object activityManager;
    private final Resources resources;
    private final Resources.Theme theme;
    private final PackageManager packageManager;
    private final Map<String, ProbeSharedPreferences> sharedPreferences = new HashMap<>();
    private final ShortcutManager shortcutManager;
    private final UserManager userManager;
    private final IBinder activityToken = new Binder();
    private Object mediaRouter;
    private Object mediaSessionManager;
    private Object accountManager;
    private Object cameraManager;
    private Object telephonyManager;
    private Object jobScheduler;
    private final String packageName;
    private final ClassLoader classLoader;
    private volatile Display display;
    private volatile Context applicationContext;
    private int nextAutofillId = 1;
    private static volatile ClassLoader applicationClassLoader;
    private final Map<ComponentName, LocalServiceRecord> localServices = new HashMap<>();
    private final Map<ServiceConnection, BoundServiceRecord> serviceConnections =
            new HashMap<>();
    private final List<ComponentCallbacks> componentCallbacks = new ArrayList<>();

    private static final class LocalServiceRecord {
        final ComponentName component;
        final Service service;
        int nextStartId = 1;
        IBinder binder;

        LocalServiceRecord(ComponentName component, Service service) {
            this.component = component;
            this.service = service;
        }
    }

    private static final class BoundServiceRecord {
        final ComponentName component;
        final LocalServiceRecord local;
        final IBinder binder;
        final int hostPid;
        final int controlFd;
        final String isolatedInstanceName;
        final String processName;

        BoundServiceRecord(ComponentName component, LocalServiceRecord local, IBinder binder,
                int hostPid, int controlFd, String isolatedInstanceName, String processName) {
            this.component = component;
            this.local = local;
            this.binder = binder;
            this.hostPid = hostPid;
            this.controlFd = controlFd;
            this.isolatedInstanceName = isolatedInstanceName;
            this.processName = processName;
        }
    }

    private static native long[] nativeSpawnService(
            String component, String instanceName, String processName, boolean isolated,
            Intent intent);
    private static native int nativeReleaseRemoteService(int hostPid, int controlFd);

    private static final class MainExecutor implements Executor {
        @Override
        public void execute(Runnable command) {
            if (command == null) throw new NullPointerException("command");
            // ActivityManager publishes service connections through the
            // target Looper after bindService() has returned, even when the
            // caller is already on that Looper.
            new Handler(Looper.getMainLooper()).post(command);
        }
    }

    private final Executor mainExecutor = new MainExecutor();

    public ProbeContext(Resources resources, PackageManager packageManager) {
        this(resources, packageManager, "dev.darwinart.probe");
    }

    public ProbeContext(Resources resources, PackageManager packageManager,
            String packageName) {
        super(new BaseContext());
        ((BaseContext) getBaseContext()).attach(this);
        this.resources = resources;
        theme = resources.newTheme();
        this.packageManager = packageManager;
        this.packageName = packageName == null ? "dev.darwinart.probe" : packageName;
        shortcutManager = new ProbeShortcutManager(this);
        userManager = constructUserManager();
        // Capture the PathClassLoader at context construction.  The detached
        // host may later execute Activity/View work on a dedicated Java UI
        // thread, so consulting that thread's mutable context loader would
        // otherwise make LayoutInflater lose APK-owned custom views.
        classLoader = ProbeContext.class.getClassLoader();
        try {
            applicationInfo = packageManager.getApplicationInfo(this.packageName, 0);
        } catch (PackageManager.NameNotFoundException missing) {
            throw new IllegalStateException("Context package is not installed", missing);
        }
        attributionSource = new AttributionSource.Builder(1000)
                .setPackageName(getPackageName())
                .build();
        contentResolver = new ProbeContentResolver(this);
        activityManager = constructActivityManager();
    }

    @Override
    public ApplicationInfo getApplicationInfo() {
        return applicationInfo;
    }

    @Override
    public Context createPackageContext(String requestedPackage, int flags)
            throws PackageManager.NameNotFoundException {
        if (packageName.equals(requestedPackage)) return this;
        // PackageManager owns package visibility.  A detached profile does
        // not provide a Google Play Services package, so report the normal
        // Android absence instead of inheriting MockContext's Stub! method
        // (which would turn an optional ProviderInstaller probe into a fatal
        // application exception).
        throw new PackageManager.NameNotFoundException(requestedPackage);
    }

    /** Applies the manifest compatibility level before Application.onCreate(). */
    public void setTargetSdkVersion(int targetSdkVersion) {
        applicationInfo.targetSdkVersion = targetSdkVersion;
    }

    /** Installs the target-SDK compatibility delegate normally owned by ActivityThread. */
    public static void configureCompatibility(int targetSdkVersion) {
        try {
            Class<?> compatibility = Class.forName("android.compat.Compatibility");
            Class<?> delegate = Class.forName(
                    "android.compat.Compatibility$BehaviorChangeDelegate");
            Object callbacks = Proxy.newProxyInstance(
                    delegate.getClassLoader(), new Class<?>[] {delegate},
                    new CompatibilityHandler(targetSdkVersion));
            Method install = compatibility.getMethod(
                    "setBehaviorChangeDelegate", delegate);
            install.invoke(null, callbacks);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not install compatibility callbacks", error);
        }
    }

    private static final class CompatibilityHandler implements InvocationHandler {
        private final int targetSdkVersion;

        CompatibilityHandler(int targetSdkVersion) {
            this.targetSdkVersion = targetSdkVersion;
        }

        @Override
        public Object invoke(Object proxy, Method method, Object[] args) {
            if ("isChangeEnabled".equals(method.getName())
                    && args != null && args.length == 1 && args[0] instanceof Long) {
                long changeId = (Long) args[0];
                if (changeId == 160794467L) return targetSdkVersion > 30;
                if (changeId == 236704164L) return targetSdkVersion >= 34;
                if (changeId == 320664730L) return targetSdkVersion > 34;
                return true;
            }
            return null;
        }
    }

    private static final class DefaultServiceHandler implements InvocationHandler {
        @Override
        public Object invoke(Object proxy, Method method, Object[] args) {
            if ("isInteractive".equals(method.getName())
                    || "isDisplayInteractive".equals(method.getName())) {
                return Boolean.TRUE;
            }
            Class<?> type = method.getReturnType();
            if (!type.isPrimitive()) return null;
            if (type == boolean.class) return Boolean.FALSE;
            if (type == byte.class) return Byte.valueOf((byte) 0);
            if (type == short.class) return Short.valueOf((short) 0);
            if (type == char.class) return Character.valueOf((char) 0);
            if (type == int.class) return Integer.valueOf(0);
            if (type == long.class) return Long.valueOf(0L);
            if (type == float.class) return Float.valueOf(0.0f);
            if (type == double.class) return Double.valueOf(0.0d);
            return null;
        }
    }

    private static final class ThermalServiceHandler implements InvocationHandler {
        @Override
        public Object invoke(Object proxy, Method method, Object[] args) {
            String name = method.getName();
            if ("registerThermalStatusListener".equals(name)
                    || "unregisterThermalStatusListener".equals(name)
                    || "registerThermalHeadroomListener".equals(name)
                    || "unregisterThermalHeadroomListener".equals(name)) {
                return Boolean.TRUE;
            }
            if ("getCurrentThermalStatus".equals(name)) return Integer.valueOf(0);
            Class<?> type = method.getReturnType();
            if (!type.isPrimitive()) return null;
            if (type == boolean.class) return Boolean.FALSE;
            if (type == byte.class) return Byte.valueOf((byte) 0);
            if (type == short.class) return Short.valueOf((short) 0);
            if (type == char.class) return Character.valueOf((char) 0);
            if (type == int.class) return Integer.valueOf(0);
            if (type == long.class) return Long.valueOf(0L);
            if (type == float.class) return Float.valueOf(0.0f);
            if (type == double.class) return Double.valueOf(0.0d);
            return null;
        }
    }

    @Override
    public Context getApplicationContext() {
        Context installed = applicationContext;
        return installed == null ? this : installed;
    }

    /** Installs the manifest Application identity normally owned by LoadedApk. */
    public void setApplicationContext(Context application) {
        if (application == null) throw new NullPointerException("application");
        applicationContext = application;
    }

    @Override
    public void startActivity(Intent intent) {
        startActivity(intent, null);
    }

    @Override
    public void startActivity(Intent intent, Bundle options) {
        if (intent == null) throw new NullPointerException("intent");
        throw new UnsupportedOperationException(
                "ProbeContext does not own ActivityTaskManager lifecycle");
    }

    @Override
    public void startActivities(Intent[] intents) {
        startActivities(intents, null);
    }

    @Override
    public void startActivities(Intent[] intents, Bundle options) {
        if (intents == null) throw new NullPointerException("intents");
        for (Intent intent : intents) startActivity(intent, options);
    }

    @Override
    public ComponentName startService(Intent intent) {
        LocalServiceRecord record = ensureLocalService(intent);
        record.service.onStartCommand(new Intent(intent), 0, record.nextStartId++);
        return record.component;
    }

    @Override
    public ComponentName startForegroundService(Intent intent) {
        try {
            ComponentName component = startService(intent);
            android.util.Log.i("ProbeContext",
                    "started local foreground Service " + component);
            return component;
        } catch (RuntimeException error) {
            android.util.Log.e("ProbeContext",
                    "could not start local foreground Service", error);
            throw error;
        }
    }

    @Override
    public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
        return bindServiceInternal(intent, connection, mainExecutor, null);
    }

    @Override
    public boolean bindService(Intent intent, int flags, Executor executor,
            ServiceConnection connection) {
        return bindServiceInternal(intent, connection, executor, null);
    }

    @Override
    public boolean bindIsolatedService(Intent intent, int flags, String instanceName,
            Executor executor, ServiceConnection connection) {
        if (instanceName == null || instanceName.isEmpty()) {
            throw new IllegalArgumentException("isolated service instance name is empty");
        }
        return bindServiceInternal(intent, connection, executor, instanceName);
    }

    @Override
    public boolean bindServiceAsUser(Intent intent, ServiceConnection connection, int flags,
            UserHandle user) {
        return bindServiceInternal(intent, connection, mainExecutor, null);
    }

    // Hidden Context API used reflectively by Chromium's process launcher.
    public boolean bindServiceAsUser(Intent intent, ServiceConnection connection, int flags,
            Handler handler, UserHandle user) {
        Executor executor = command -> handler.post(command);
        return bindServiceInternal(intent, connection, executor, null);
    }

    @Override
    public void updateServiceGroup(ServiceConnection connection, int group, int importance) {
        if (!serviceConnections.containsKey(connection)) {
            throw new IllegalArgumentException("Service not registered");
        }
    }

    private boolean bindServiceInternal(Intent intent, ServiceConnection connection,
            Executor executor, String isolatedInstanceName) {
        if (connection == null) throw new NullPointerException("connection");
        if (executor == null) throw new NullPointerException("executor");
        ComponentName component = intent == null ? null : intent.getComponent();
        if (component == null || !packageName.equals(component.getPackageName())) {
            // A detached profile has no Play Services/system-server process to
            // bind. Android reports an unavailable explicit service as a
            // failed bind; do not turn that normal capability miss into an
            // uncaught exception on the app's main looper.
            android.util.Log.w("ProbeContext",
                    "service unavailable outside profile package: "
                            + (component == null ? "<implicit>" : component));
            return false;
        }
        if (isolatedInstanceName != null) {
            // ActivityManager identifies an isolated service by component and
            // instance name. Chromium deliberately creates several binding
            // connections (strong/visible/waive-priority) to that one service;
            // each bind must receive the same process and Binder endpoint.
            for (BoundServiceRecord existing : serviceConnections.values()) {
                if (existing.local == null
                        && component.equals(existing.component)
                        && isolatedInstanceName.equals(existing.isolatedInstanceName)) {
                    BoundServiceRecord rebound = new BoundServiceRecord(
                            component, null, existing.binder, existing.hostPid,
                            existing.controlFd, isolatedInstanceName, existing.processName);
                    serviceConnections.put(connection, rebound);
                    android.util.Log.i("ProbeContext",
                            "rebind isolated Service " + component + " instance="
                                    + isolatedInstanceName + " pid=" + existing.hostPid);
                    dispatchServiceConnected(
                            executor, connection, component, existing.binder);
                    return true;
                }
            }
            String processName = resolveServiceProcessName(component, isolatedInstanceName);
            long[] child = nativeSpawnService(
                    component.flattenToString(), isolatedInstanceName, processName, true, intent);
            if (child == null || child.length != 3 || child[0] <= 0
                    || child[0] > Integer.MAX_VALUE || child[1] < 0
                    || child[1] > Integer.MAX_VALUE || child[2] == 0) {
                android.util.Log.e("ProbeContext",
                        "could not spawn isolated Service " + component);
                return false;
            }
            final int hostPid = (int) child[0];
            final int controlFd = (int) child[1];
            final long channelGeneration = child[2];
            final IBinder binder;
            try {
                binder = new RemoteBinder(controlFd, 1, channelGeneration);
            } catch (RuntimeException error) {
                android.util.Log.w("ProbeContext",
                        "isolated Service endpoint failed before ready: " + component
                                + " instance=" + isolatedInstanceName, error);
                nativeReleaseRemoteService(hostPid, controlFd);
                return false;
            }
            BoundServiceRecord bound = new BoundServiceRecord(
                    component, null, binder, hostPid, controlFd, isolatedInstanceName,
                    processName);
            serviceConnections.put(connection, bound);
            android.util.Log.i("ProbeContext",
                    "bind isolated Service " + component + " instance="
                            + isolatedInstanceName + " process=" + processName
                            + " pid=" + child[0]);
            dispatchServiceConnected(executor, connection, component, binder);
            return true;
        }
        String processName = resolveDeclaredServiceProcessName(component);
        String currentProcessName = Application.getProcessName();
        if (currentProcessName == null || currentProcessName.isEmpty()) {
            currentProcessName = packageName;
        }
        if (!currentProcessName.equals(processName)) {
            // A service with android:process is owned by ActivityManager in a
            // distinct app process even when it is not isolated. Chromium's
            // GPU service uses exactly this bindServiceAsUser path.
            for (BoundServiceRecord existing : serviceConnections.values()) {
                if (existing.local == null
                        && existing.isolatedInstanceName == null
                        && component.equals(existing.component)
                        && processName.equals(existing.processName)) {
                    BoundServiceRecord rebound = new BoundServiceRecord(
                            component, null, existing.binder, existing.hostPid,
                            existing.controlFd, null, processName);
                    serviceConnections.put(connection, rebound);
                    android.util.Log.i("ProbeContext",
                            "rebind remote Service " + component + " process="
                                    + processName + " pid=" + existing.hostPid);
                    dispatchServiceConnected(executor, connection, component, existing.binder);
                    return true;
                }
            }
            long[] child = nativeSpawnService(
                    component.flattenToString(), "", processName, false, intent);
            if (child == null || child.length != 3 || child[0] <= 0
                    || child[0] > Integer.MAX_VALUE || child[1] < 0
                    || child[1] > Integer.MAX_VALUE || child[2] == 0) {
                android.util.Log.e("ProbeContext",
                        "could not spawn remote Service " + component);
                return false;
            }
            final int hostPid = (int) child[0];
            final int controlFd = (int) child[1];
            final long channelGeneration = child[2];
            final IBinder binder;
            try {
                binder = new RemoteBinder(controlFd, 1, channelGeneration);
            } catch (RuntimeException error) {
                android.util.Log.w("ProbeContext",
                        "remote Service endpoint failed before ready: " + component, error);
                nativeReleaseRemoteService(hostPid, controlFd);
                return false;
            }
            serviceConnections.put(connection, new BoundServiceRecord(
                    component, null, binder, hostPid, controlFd, null, processName));
            android.util.Log.i("ProbeContext",
                    "bind remote Service " + component + " process=" + processName
                            + " pid=" + child[0]);
            dispatchServiceConnected(executor, connection, component, binder);
            return true;
        }
        LocalServiceRecord record = ensureLocalService(intent);
        if (record.binder == null) record.binder = record.service.onBind(new Intent(intent));
        android.util.Log.i("ProbeContext",
                "bind local Service " + record.component + " action=" + intent.getAction()
                        + " isolated=" + isolatedInstanceName + " binder=" + record.binder);
        if (record.binder == null) return false;
        serviceConnections.put(connection, new BoundServiceRecord(
                record.component, record, record.binder, -1, -1, null, packageName));
        // ActivityManager reports service connections asynchronously after
        // bindService() returns. MediaBrowser (and many AndroidX clients)
        // establishes its CONNECTING state only after that return, so a
        // synchronous callback is observably invalid even in-process.
        dispatchServiceConnected(executor, connection, record.component, record.binder);
        return true;
    }

    private void dispatchServiceConnected(Executor executor, ServiceConnection connection,
            ComponentName component, IBinder binder) {
        Runnable connected = () -> {
            BoundServiceRecord current = serviceConnections.get(connection);
            if (current == null || current.binder != binder) return;
            android.util.Log.i("ProbeContext", "service connection dispatch " + component
                    + " connection=" + connection.getClass().getName()
                    + " thread=" + Thread.currentThread().getName());
            try {
                connection.onServiceConnected(component, binder);
            } finally {
                android.util.Log.i("ProbeContext", "service connection completed " + component
                        + " connection=" + connection.getClass().getName()
                        + " thread=" + Thread.currentThread().getName());
            }
        };
        if (binder instanceof RemoteBinder) {
            serviceReadiness.execute(() -> {
                try {
                    ((RemoteBinder) binder).awaitReady();
                    executor.execute(connected);
                } catch (RuntimeException error) {
                    android.util.Log.w("ProbeContext",
                            "Service failed before connection: " + component, error);
                    executor.execute(() -> {
                        BoundServiceRecord current = serviceConnections.get(connection);
                        if (current == null || current.binder != binder) return;
                        unbindService(connection);
                        connection.onBindingDied(component);
                    });
                }
            });
        } else {
            executor.execute(connected);
        }
    }

    @Override
    public void unbindService(ServiceConnection connection) {
        BoundServiceRecord record = serviceConnections.remove(connection);
        if (record == null) throw new IllegalArgumentException("Service not registered");
        if (record.local != null) {
            record.local.service.onUnbind(new Intent().setComponent(record.component));
        } else if (serviceConnections.values().stream().noneMatch(
                existing -> existing.local == null
                        && existing.hostPid == record.hostPid
                        && existing.controlFd == record.controlFd)
                && nativeReleaseRemoteService(record.hostPid, record.controlFd) != 0) {
            throw new IllegalStateException(
                    "Could not release remote Service pid=" + record.hostPid);
        }
    }

    @Override
    public boolean stopService(Intent intent) {
        ComponentName component = requireServiceComponent(intent);
        LocalServiceRecord record = localServices.remove(component);
        if (record == null) return false;
        serviceConnections.entrySet().removeIf(entry -> entry.getValue().local == record);
        record.service.onDestroy();
        return true;
    }

    private LocalServiceRecord ensureLocalService(Intent intent) {
        ComponentName component = requireServiceComponent(intent);
        LocalServiceRecord existing = localServices.get(component);
        if (existing != null) return existing;
        try {
            Class<?> serviceClass = Class.forName(
                    component.getClassName(), true, getClassLoader());
            Service service = (Service) serviceClass.getDeclaredConstructor().newInstance();
            Class<?> activityThreadClass = Class.forName("android.app.ActivityThread");
            Method attach = Service.class.getDeclaredMethod("attach",
                    Context.class, activityThreadClass, String.class,
                    IBinder.class, Application.class, Object.class);
            attach.setAccessible(true);
            Context installed = getApplicationContext();
            Application application = installed instanceof Application
                    ? (Application) installed : null;
            if (application == null) {
                throw new IllegalStateException("Application context is not installed");
            }
            // Service.attach receives the process IActivityManager directly on
            // Android. Passing null leaves startForeground/stopSelf unusable
            // even though ServiceManager already exposes our typed local
            // activity service, so resolve the same framework singleton here.
            Method getActivityManager = android.app.ActivityManager.class
                    .getDeclaredMethod("getService");
            getActivityManager.setAccessible(true);
            Object activityManagerService = getActivityManager.invoke(null);
            if (activityManagerService == null) {
                throw new IllegalStateException("IActivityManager is not installed");
            }
            attach.invoke(service, this, null, component.getClassName(),
                    new Binder(), application, activityManagerService);
            service.onCreate();
            LocalServiceRecord created = new LocalServiceRecord(component, service);
            localServices.put(component, created);
            return created;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not create local Service " + component,
                    error);
        }
    }

    private ComponentName requireServiceComponent(Intent intent) {
        ComponentName component = intent == null ? null : intent.getComponent();
        if (component == null || !packageName.equals(component.getPackageName())) {
            throw new IllegalArgumentException("Only explicit in-package Services are supported");
        }
        return component;
    }

    private String resolveServiceProcessName(ComponentName component, String instanceName) {
        return resolveDeclaredServiceProcessName(component) + ":" + instanceName;
    }

    private String resolveDeclaredServiceProcessName(ComponentName component) {
        String declared = null;
        try {
            declared = packageManager.getServiceInfo(component, 0).processName;
        } catch (PackageManager.NameNotFoundException ignored) {
        }
        if (declared == null || declared.isEmpty()) {
            String className = component.getClassName();
            int separator = className.lastIndexOf('.');
            declared = packageName + ":"
                    + (separator < 0 ? className : className.substring(separator + 1));
        }
        return declared;
    }

    @Override
    public Context createDeviceProtectedStorageContext() {
        return this;
    }

    @Override
    public Context createDeviceContext(int deviceId) {
        if (deviceId != 0) {
            throw new IllegalArgumentException("Only the default Android device is available");
        }
        return this;
    }

    @Override
    public Context createConfigurationContext(Configuration overrideConfiguration) {
        // The detached owner exposes one display and one Resources instance.
        // PhoneWindow asks its application context for an equivalent display
        // configuration while constructing DecorContext; the already-applied
        // resource configuration is authoritative for that display.
        return this;
    }

    @Override
    public Context createDisplayContext(Display requestedDisplay) {
        if (requestedDisplay == null || requestedDisplay.getDisplayId() != 0) {
            throw new IllegalArgumentException("Only the primary Android display is available");
        }
        return this;
    }

    @Override
    public Context createWindowContext(Display requestedDisplay, int type, Bundle options) {
        return createDisplayContext(requestedDisplay);
    }

    @Override
    public Context createWindowContext(int type, Bundle options) {
        return this;
    }

    // Hidden framework API on the public SDK; ART still dispatches the virtual
    // Context.getActivityToken() call to this compatible signature.
    public IBinder getActivityToken() {
        return activityToken;
    }

    // An Activity-backed context is not a WindowContext. ContextImpl returns
    // null here; ViewRoot uses it to route configuration callbacks correctly.
    public IBinder getWindowContextToken() {
        return null;
    }

    @Override
    public synchronized void registerComponentCallbacks(ComponentCallbacks callback) {
        if (callback == null) throw new NullPointerException("callback");
        if (!componentCallbacks.contains(callback)) componentCallbacks.add(callback);
    }

    @Override
    public synchronized void unregisterComponentCallbacks(ComponentCallbacks callback) {
        componentCallbacks.remove(callback);
    }

    @Override
    public boolean isDeviceProtectedStorage() {
        // The profile data directory is already unlocked before an app is
        // launched. WorkManager and AndroidX Startup must therefore observe
        // credential-protected storage, just as they do after user unlock on
        // a physical Android device.
        return false;
    }

    @Override
    public boolean moveSharedPreferencesFrom(Context sourceContext, String name) {
        return true;
    }

    @Override
    public synchronized SharedPreferences getSharedPreferences(String name, int mode) {
        if (name == null) throw new NullPointerException("name");
        ProbeSharedPreferences preferences = sharedPreferences.get(name);
        if (preferences == null) {
            File directory = new File(getDataDir(), "shared_prefs");
            preferences = new ProbeSharedPreferences(new File(directory, name + ".xml"));
            sharedPreferences.put(name, preferences);
        }
        return preferences;
    }

    @Override
    public synchronized boolean deleteSharedPreferences(String name) {
        if (name == null) throw new NullPointerException("name");
        sharedPreferences.remove(name);
        File file = new File(new File(getDataDir(), "shared_prefs"), name + ".xml");
        File backup = new File(file.getPath() + ".bak");
        boolean deleted = !file.exists() || file.delete();
        return (!backup.exists() || backup.delete()) && deleted;
    }

    private File appDataPath(String child) {
        String configured = System.getenv("DARWIN_ART_APK_APP_DATA_DIR");
        File hostRoot = new File(configured == null
                ? new File(System.getProperty("java.io.tmpdir"), "darwin-art-app-data")
                        .getPath()
                : configured);
        File hostResult = child == null ? hostRoot : new File(hostRoot, child);
        hostResult.mkdirs();
        String guest = System.getenv("DARWIN_ART_APK_APP_DATA_GUEST_DIR");
        File guestRoot = new File(guest == null || guest.isEmpty()
                ? hostRoot.getPath() : guest);
        File guestResult = child == null ? guestRoot : new File(guestRoot, child);
        guestResult.mkdirs();
        return guestResult;
    }

    @Override
    public File getDataDir() {
        return appDataPath(null);
    }

    @Override
    public File getFilesDir() {
        return appDataPath("files");
    }

    private File makeFilename(File base, String name) {
        if (name.indexOf(File.separatorChar) >= 0) {
            throw new IllegalArgumentException("File " + name + " contains a path separator");
        }
        return new File(base, name);
    }

    @Override
    public FileInputStream openFileInput(String name) throws FileNotFoundException {
        return new FileInputStream(makeFilename(getFilesDir(), name));
    }

    @Override
    public FileOutputStream openFileOutput(String name, int mode) throws FileNotFoundException {
        boolean append = (mode & MODE_APPEND) != 0;
        return new FileOutputStream(makeFilename(getFilesDir(), name), append);
    }

    @Override
    public boolean deleteFile(String name) {
        return makeFilename(getFilesDir(), name).delete();
    }

    @Override
    public File getFileStreamPath(String name) {
        return makeFilename(getFilesDir(), name);
    }

    @Override
    public String[] fileList() {
        String[] files = getFilesDir().list();
        return files == null ? new String[0] : files;
    }

    @Override
    public File getCacheDir() {
        return appDataPath("cache");
    }

    @Override
    public File getCodeCacheDir() {
        return appDataPath("code_cache");
    }

    @Override
    public File getNoBackupFilesDir() {
        return appDataPath("no_backup");
    }

    @Override
    public File getExternalFilesDir(String type) {
        return appDataPath(type == null || type.isEmpty()
                ? "external/files" : "external/files/" + type);
    }

    @Override
    public File[] getExternalFilesDirs(String type) {
        return new File[] {getExternalFilesDir(type)};
    }

    /**
     * OBB storage is external shared storage, not the app-private external
     * files subtree. ContextImpl exposes one package-scoped directory at
     * /storage/emulated/0/Android/obb/<package>; keep the guest path visible to
     * Java while mirroring it into this profile's writable host capability.
     */
    private File obbPath() {
        String configuredExternal = System.getenv("DARWIN_ART_APK_APP_EXTERNAL_DIR");
        String guestRoot = "/storage/emulated/0";
        if (configuredExternal != null && !configuredExternal.isEmpty()) {
            int marker = configuredExternal.indexOf("/Android/data/");
            if (marker > 0) guestRoot = configuredExternal.substring(0, marker);
        }
        String relative = "Android/obb/" + packageName;
        File hostData = new File(System.getenv("DARWIN_ART_APK_APP_DATA_DIR") == null
                ? new File(System.getProperty("java.io.tmpdir"), "darwin-art-app-data").getPath()
                : System.getenv("DARWIN_ART_APK_APP_DATA_DIR"));
        File hostResult = new File(new File(hostData, "external"), relative);
        hostResult.mkdirs();
        File guestResult = new File(new File(guestRoot), relative);
        guestResult.mkdirs();
        return guestResult;
    }

    @Override
    public File getObbDir() {
        return obbPath();
    }

    @Override
    public File[] getObbDirs() {
        return new File[] {getObbDir()};
    }

    @Override
    public File getExternalCacheDir() {
        return appDataPath("external/cache");
    }

    @Override
    public File[] getExternalCacheDirs() {
        return new File[] {getExternalCacheDir()};
    }

    @Override
    public File getDir(String name, int mode) {
        if (name == null || name.isEmpty() || name.indexOf('/') >= 0
                || name.indexOf('\\') >= 0) {
            throw new IllegalArgumentException("Invalid app directory name: " + name);
        }
        return appDataPath("app_" + name);
    }

    @Override
    public File getDatabasePath(String name) {
        File databases = appDataPath("databases");
        return new File(databases, name);
    }

    @Override
    public ContentResolver getContentResolver() {
        return contentResolver;
    }

    @Override
    public Intent registerReceiver(BroadcastReceiver receiver, IntentFilter filter) {
        // A detached app process has no ActivityManager broadcast registry.
        // Registration succeeds locally; no sticky broadcast is pending.
        return null;
    }

    @Override
    public Intent registerReceiver(BroadcastReceiver receiver, IntentFilter filter, int flags) {
        return registerReceiver(receiver, filter);
    }

    @Override
    public Intent registerReceiver(BroadcastReceiver receiver, IntentFilter filter,
            String broadcastPermission, Handler scheduler) {
        return registerReceiver(receiver, filter);
    }

    @Override
    public Intent registerReceiver(BroadcastReceiver receiver, IntentFilter filter,
            String broadcastPermission, Handler scheduler, int flags) {
        return registerReceiver(receiver, filter);
    }

    public Intent registerReceiverAsUser(BroadcastReceiver receiver, UserHandle user,
            IntentFilter filter, String broadcastPermission, Handler scheduler) {
        return null;
    }

    @Override
    public void unregisterReceiver(BroadcastReceiver receiver) {}

    /**
     * Delivers the process-local portion of Android's broadcast contract.
     * The detached runtime has no system_server broadcast queue, but sending
     * must remain a valid operation: Chrome and framework components use it
     * for best-effort lifecycle/metrics notifications during startup.
     */
    @Override
    public void sendBroadcast(Intent intent) {}

    @Override
    public void sendBroadcast(Intent intent, String receiverPermission) {}

    @Override
    public void sendBroadcastAsUser(Intent intent, UserHandle user) {}

    @Override
    public AttributionSource getAttributionSource() {
        return attributionSource;
    }

    @Override
    public Resources getResources() {
        return resources;
    }

    @Override
    public android.content.res.AssetManager getAssets() {
        return resources.getAssets();
    }

    @Override
    public Resources.Theme getTheme() {
        return theme;
    }

    @Override
    public PackageManager getPackageManager() {
        return packageManager;
    }

    @Override
    public int checkPermission(String permission, int pid, int uid) {
        if (permission == null || pid != Process.myPid() || uid != Process.myUid()) {
            return PackageManager.PERMISSION_DENIED;
        }
        // Android shared storage is a runtime-owned, per-app virtual volume in
        // this host, so its legacy and media-scoped permissions are safe to
        // grant at install time. Hardware and host-resource capabilities stay
        // denied until their permission controller is connected.
        if ("android.permission.CAMERA".equals(permission)
                || "android.permission.RECORD_AUDIO".equals(permission)
                || "android.permission.ACCESS_FINE_LOCATION".equals(permission)
                || "android.permission.ACCESS_COARSE_LOCATION".equals(permission)
                || "android.permission.ACCESS_BACKGROUND_LOCATION".equals(permission)
                || "android.permission.ACCESS_MEDIA_LOCATION".equals(permission)) {
            return PackageManager.PERMISSION_DENIED;
        }
        return PackageManager.PERMISSION_GRANTED;
    }

    @Override
    public int checkCallingOrSelfPermission(String permission) {
        return checkPermission(permission, Process.myPid(), Process.myUid());
    }

    @Override
    public int checkSelfPermission(String permission) {
        return checkPermission(permission, Process.myPid(), Process.myUid());
    }

    @Override
    public Looper getMainLooper() {
        return Looper.getMainLooper();
    }

    @Override
    public Executor getMainExecutor() {
        return mainExecutor;
    }

    /** ContextImpl's display identity for PhoneWindow / DecorContext. */
    public Display getDisplayNoVerify() {
        Display current = display;
        if (current != null) return current;
        try {
            Class<?> globalType = Class.forName(
                    "android.hardware.display.DisplayManagerGlobal");
            Method getInstance = globalType.getDeclaredMethod("getInstance");
            getInstance.setAccessible(true);
            Object global = getInstance.invoke(null);
            Method getRealDisplay = globalType.getDeclaredMethod(
                    "getRealDisplay", int.class);
            getRealDisplay.setAccessible(true);
            current = (Display) getRealDisplay.invoke(global, Integer.valueOf(0));
            if (current == null) {
                throw new IllegalStateException("Primary Android display is unavailable");
            }
            display = current;
            return current;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not resolve primary Android display", error);
        }
    }

    @Override
    public Display getDisplay() {
        return getDisplayNoVerify();
    }

    /** Hidden Context virtual used by ViewConfiguration on Android 16. */
    public int getDisplayId() {
        return 0;
    }

    /** Hidden ContextImpl hook used by DecorContext during window creation. */
    public void updateDisplay(int displayId) {
        if (displayId != 0) {
            throw new IllegalArgumentException("Only the primary Android display is available");
        }
        getDisplayNoVerify();
    }

    @Override
    public Object getSystemService(String name) {
        if (ACTIVITY_SERVICE.equals(name)) {
            return activityManager;
        }
        if (APP_OPS_SERVICE.equals(name)) {
            return constructAppOpsManager();
        }
        if (INPUT_METHOD_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName("android.view.inputmethod.InputMethodManager");
                // ContextImpl routes this lookup through SystemServiceRegistry,
                // which in turn calls InputMethodManager.forContext().  Reuse
                // that display-scoped framework singleton: ViewRootImpl owns
                // its focus/served-view state, so manufacturing a second IMM
                // here leaves application calls disconnected from the real
                // window even when physical key events reach the ViewRoot.
                java.lang.reflect.Method factory = type.getDeclaredMethod(
                        "forContext", Context.class);
                factory.setAccessible(true);
                return factory.invoke(null, this);
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException("Could not resolve input method manager", error);
            }
        }
        if (TEXT_SERVICES_MANAGER_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName(
                        "android.view.textservice.TextServicesManager");
                Method factory = type.getMethod("createInstance", Context.class);
                return factory.invoke(null, this);
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException(
                        "Could not construct text services manager", error);
            }
        }
        if (INPUT_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName("android.hardware.input.InputManager");
                Constructor<?> constructor = type.getDeclaredConstructor(Context.class);
                constructor.setAccessible(true);
                return constructor.newInstance(this);
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException("Could not construct input manager", error);
            }
        }
        if (UI_MODE_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName("android.app.UiModeManager");
                Constructor<?> constructor = type.getDeclaredConstructor(Context.class);
                constructor.setAccessible(true);
                return constructor.newInstance(this);
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException("Could not construct UI mode manager", error);
            }
        }
        if (BLUETOOTH_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName("android.bluetooth.BluetoothManager");
                Constructor<?> constructor = type.getDeclaredConstructor(Context.class);
                constructor.setAccessible(true);
                return constructor.newInstance(this);
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException("Could not construct Bluetooth manager", error);
            }
        }
        if (LAYOUT_INFLATER_SERVICE.equals(name)) {
            return construct("com.android.internal.policy.PhoneLayoutInflater");
        }
        if (WINDOW_SERVICE.equals(name)) {
            return construct("android.view.WindowManagerImpl");
        }
        if (DISPLAY_SERVICE.equals(name)) {
            return construct("android.hardware.display.DisplayManager");
        }
        if (ACCESSIBILITY_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName(
                        "android.view.accessibility.AccessibilityManager");
                Method factory = type.getMethod("getInstance", Context.class);
                return factory.invoke(null, this);
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException(
                        "Could not construct accessibility manager", error);
            }
        }
        if (CAPTIONING_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName(
                        "android.view.accessibility.CaptioningManager");
                Constructor<?> constructor = type.getDeclaredConstructor(Context.class);
                constructor.setAccessible(true);
                return constructor.newInstance(this);
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException(
                        "Could not construct captioning manager", error);
            }
        }
        if (SHORTCUT_SERVICE.equals(name)) {
            return shortcutManager;
        }
        if (NOTIFICATION_SERVICE.equals(name)) {
            return construct("android.app.NotificationManager");
        }
        if (USER_SERVICE.equals(name)) {
            return userManager;
        }
        if (DEVICE_POLICY_SERVICE.equals(name)) {
            return constructDevicePolicyManager();
        }
        if (USAGE_STATS_SERVICE.equals(name)) {
            return constructUsageStatsManager();
        }
        if (JOB_SCHEDULER_SERVICE.equals(name)) {
            if (jobScheduler == null) jobScheduler = constructJobScheduler();
            return jobScheduler;
        }
        if (VIBRATOR_SERVICE.equals(name)) {
            return construct("android.os.SystemVibrator");
        }
        if (ALARM_SERVICE.equals(name)) {
            return constructProxyManager("android.app.AlarmManager",
                    "android.app.IAlarmManager");
        }
        if (POWER_SERVICE.equals(name)) {
            return constructPowerManager();
        }
        if (AUDIO_SERVICE.equals(name)) {
            return new ProbeAudioManager(this);
        }
        if (MEDIA_ROUTER_SERVICE.equals(name)) {
            if (mediaRouter == null) {
                mediaRouter = construct("android.media.MediaRouter");
            }
            return mediaRouter;
        }
        if (MEDIA_SESSION_SERVICE.equals(name)) {
            return constructMediaSessionManager();
        }
        if (KEYGUARD_SERVICE.equals(name)) {
            return construct("android.app.KeyguardManager");
        }
        if (CONNECTIVITY_SERVICE.equals(name)) {
            return construct("android.net.ConnectivityManager");
        }
        if (STORAGE_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName("android.os.storage.StorageManager");
                Constructor<?> constructor = type.getDeclaredConstructor(
                        Context.class, Looper.class);
                constructor.setAccessible(true);
                return constructor.newInstance(this, Looper.getMainLooper());
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException(
                        "Could not construct storage manager", error);
            }
        }
        if (CLIPBOARD_SERVICE.equals(name)) {
            try {
                Class<?> type = Class.forName("android.content.ClipboardManager");
                Constructor<?> constructor = type.getDeclaredConstructor(
                        Context.class, Handler.class);
                constructor.setAccessible(true);
                return constructor.newInstance(this, new Handler(Looper.getMainLooper()));
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException("Could not construct clipboard manager", error);
            }
        }
        if (ACCOUNT_SERVICE.equals(name)) {
            android.util.Log.i("ProbeContext",
                    "constructing framework AccountManager");
            return constructAccountManager();
        }
        if (CAMERA_SERVICE.equals(name)) {
            if (cameraManager == null) {
                cameraManager = construct("android.hardware.camera2.CameraManager");
            }
            return cameraManager;
        }
        if (TELEPHONY_SERVICE.equals(name)) {
            if (telephonyManager == null) {
                telephonyManager = construct("android.telephony.TelephonyManager");
            }
            return telephonyManager;
        }
        return null;
    }

    @Override
    public String getSystemServiceName(Class<?> serviceClass) {
        String className = serviceClass.getName();
        if ("android.app.ActivityManager".equals(className)) {
            return ACTIVITY_SERVICE;
        }
        if ("android.app.AppOpsManager".equals(className)) {
            return APP_OPS_SERVICE;
        }
        if ("android.view.inputmethod.InputMethodManager".equals(className)) {
            return INPUT_METHOD_SERVICE;
        }
        if ("android.view.textservice.TextServicesManager".equals(className)) {
            return TEXT_SERVICES_MANAGER_SERVICE;
        }
        if ("android.hardware.input.InputManager".equals(className)) {
            return INPUT_SERVICE;
        }
        if ("android.app.UiModeManager".equals(className)) {
            return UI_MODE_SERVICE;
        }
        if ("android.bluetooth.BluetoothManager".equals(className)) {
            return BLUETOOTH_SERVICE;
        }
        if ("android.view.WindowManager".equals(className)) {
            return WINDOW_SERVICE;
        }
        if ("android.view.LayoutInflater".equals(className)) {
            return LAYOUT_INFLATER_SERVICE;
        }
        if ("android.hardware.display.DisplayManager".equals(className)) {
            return DISPLAY_SERVICE;
        }
        if ("android.view.accessibility.AccessibilityManager".equals(className)) {
            return ACCESSIBILITY_SERVICE;
        }
        if ("android.view.accessibility.CaptioningManager".equals(className)) {
            return CAPTIONING_SERVICE;
        }
        if ("android.content.pm.ShortcutManager".equals(className)) {
            return SHORTCUT_SERVICE;
        }
        if ("android.app.NotificationManager".equals(className)) {
            return NOTIFICATION_SERVICE;
        }
        if ("android.content.ClipboardManager".equals(className)) {
            return CLIPBOARD_SERVICE;
        }
        if ("android.accounts.AccountManager".equals(className)) {
            return ACCOUNT_SERVICE;
        }
        if ("android.os.UserManager".equals(className)) {
            return USER_SERVICE;
        }
        if ("android.os.Vibrator".equals(className)) {
            return VIBRATOR_SERVICE;
        }
        if ("android.app.usage.UsageStatsManager".equals(className)) {
            return USAGE_STATS_SERVICE;
        }
        if (JobScheduler.class.getName().equals(className)) {
            return JOB_SCHEDULER_SERVICE;
        }
        if ("android.app.AlarmManager".equals(className)) {
            return ALARM_SERVICE;
        }
        if ("android.os.PowerManager".equals(className)) {
            return POWER_SERVICE;
        }
        if ("android.media.AudioManager".equals(className)) {
            return AUDIO_SERVICE;
        }
        if ("android.media.MediaRouter".equals(className)) {
            return MEDIA_ROUTER_SERVICE;
        }
        if ("android.media.session.MediaSessionManager".equals(className)) {
            return MEDIA_SESSION_SERVICE;
        }
        if ("android.app.KeyguardManager".equals(className)) {
            return KEYGUARD_SERVICE;
        }
        if ("android.net.ConnectivityManager".equals(className)) {
            return CONNECTIVITY_SERVICE;
        }
        if ("android.os.storage.StorageManager".equals(className)) {
            return STORAGE_SERVICE;
        }
        if ("android.hardware.camera2.CameraManager".equals(className)) {
            return CAMERA_SERVICE;
        }
        if ("android.telephony.TelephonyManager".equals(className)) {
            return TELEPHONY_SERVICE;
        }
        return null;
    }

    private Object constructActivityManager() {
        try {
            Class<?> type = Class.forName("android.app.ActivityManager");
            Constructor<?> constructor = type.getDeclaredConstructor(
                    Context.class, Handler.class);
            constructor.setAccessible(true);
            return constructor.newInstance(this, new Handler(Looper.getMainLooper()));
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not construct activity manager", error);
        }
    }

    private synchronized Object constructMediaSessionManager() {
        // ContextImpl caches service fetchers per Context. MediaFramework's
        // process initializer is deliberately one-shot, so constructing this
        // manager for every getSystemService() call breaks the second media
        // key dispatched through PhoneFallbackEventHandler.
        if (mediaSessionManager != null) return mediaSessionManager;
        try {
            Class<?> serviceManager = Class.forName("android.media.MediaServiceManager");
            Object services = serviceManager.getDeclaredConstructor().newInstance();
            Class<?> initializer = Class.forName(
                    "android.media.MediaFrameworkPlatformInitializer");
            Method install = initializer.getMethod(
                    "setMediaServiceManager", serviceManager);
            install.invoke(null, services);
            mediaSessionManager = construct("android.media.session.MediaSessionManager");
            return mediaSessionManager;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not initialize media services", error);
        }
    }

    private synchronized Object constructAccountManager() {
        if (accountManager != null) return accountManager;
        try {
            Class<?> serviceManager = Class.forName("android.os.ServiceManager");
            Method getService = serviceManager.getDeclaredMethod(
                    "getService", String.class);
            getService.setAccessible(true);
            IBinder binder = (IBinder) getService.invoke(null, ACCOUNT_SERVICE);
            if (binder == null) {
                throw new IllegalStateException("IAccountManager is not installed");
            }
            Class<?> interfaceClass = Class.forName(
                    "android.accounts.IAccountManager");
            Class<?> stubClass = Class.forName(
                    "android.accounts.IAccountManager$Stub");
            Method asInterface = stubClass.getMethod("asInterface", IBinder.class);
            Object service = asInterface.invoke(null, binder);
            Class<?> managerClass = Class.forName("android.accounts.AccountManager");
            Constructor<?> constructor = managerClass.getDeclaredConstructor(
                    Context.class, interfaceClass);
            constructor.setAccessible(true);
            accountManager = constructor.newInstance(this, service);
            android.util.Log.i("ProbeContext",
                    "framework AccountManager installed binder=" + binder);
            return accountManager;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not construct account manager", error);
        }
    }

    // Hidden Context hooks called by Activity.attachBaseContext(). The Darwin
    // host does not expose autofill or content-capture services, so ownership
    // stays with the attached Activity while these services remain disabled.
    public void setAutofillClient(AutofillManager.AutofillClient client) {}

    public void setContentCaptureOptions(ContentCaptureOptions options) {}

    /** ContextImpl's process-local virtual-id allocator used by View autofill identity. */
    public synchronized int getNextAutofillId() {
        return nextAutofillId++;
    }

    @Override
    public String getPackageName() {
        return packageName;
    }

    // ContextWrapper's implementation dereferences the null base used by
    // this detached context. Android's ViewRootImpl uses this identity while
    // constructing AttachInfo, so expose the same package-owned value.
    public String getBasePackageName() {
        return packageName;
    }

    // ProbeContext intentionally has no host ContextImpl backing object.  The
    // real Android ContextImpl nevertheless exposes the application
    // PathClassLoader, and LayoutInflater uses Context.getClassLoader() while
    // constructing custom views from an APK layout.  Return the owner thread's
    // installed app loader instead of delegating through the null ContextWrapper
    // base used by this detached Darwin context.
    @Override
    public ClassLoader getClassLoader() {
        // ContextImpl on Android returns the application PathClassLoader.  In
        // the detached launcher the support DEX and APK DEX are registered
        // together, but the support class can retain the loader that first
        // defined it.  Prefer the UI thread's installed application loader so
        // LayoutInflater resolves custom APK views (CalculatorEditText, etc.)
        // from the same loader as the Activity itself.
        ClassLoader application = applicationClassLoader;
        if (application != null) {
            return application;
        }
        ClassLoader current = Thread.currentThread().getContextClassLoader();
        return current != null ? current : classLoader;
    }

    /** Installs the Activity's complete APK+support PathClassLoader. */
    public static void installApplicationClassLoader(ClassLoader loader) {
        applicationClassLoader = loader;
    }

    @Override
    public String getOpPackageName() {
        return getPackageName();
    }

    @Override
    public String getAttributionTag() {
        return null;
    }

    @Override
    public boolean isRestricted() {
        return false;
    }

    // Hidden Context hook used while TextView resolves framework text
    // appearances. The trusted framework-res APK is the only resource owner
    // installed by this probe.
    public boolean canLoadUnsafeResources() {
        return true;
    }

    // Hidden on the SDK surface but virtual in the platform Context class.
    public int getUserId() {
        return 0;
    }

    // Android 16 associates Contexts with the default virtual device unless
    // a device-aware display context says otherwise. The Darwin host exposes
    // one display/device, so framework managers consistently use device 0.
    public int getDeviceId() {
        return 0;
    }

    // Hidden virtual Context hook used by NotificationManager.
    public UserHandle getUser() {
        return Process.myUserHandle();
    }

    private Object construct(String className) {
        try {
            Class<?> type = Class.forName(className);
            Constructor<?> constructor = type.getDeclaredConstructor(Context.class);
            constructor.setAccessible(true);
            return constructor.newInstance(this);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not construct " + className, error);
        }
    }

    private Object constructProxyManager(String className, String interfaceName) {
        try {
            Class<?> interfaceType = Class.forName(interfaceName);
            Object service = Proxy.newProxyInstance(interfaceType.getClassLoader(),
                    new Class<?>[] {interfaceType}, new DefaultServiceHandler());
            Class<?> type = Class.forName(className);
            Constructor<?> constructor = type.getDeclaredConstructor(
                    interfaceType, Context.class);
            constructor.setAccessible(true);
            return constructor.newInstance(service, this);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not construct " + className, error);
        }
    }

    private Object constructPowerManager() {
        try {
            Class<?> powerInterface = Class.forName("android.os.IPowerManager");
            Class<?> thermalInterface = Class.forName("android.os.IThermalService");
            Object powerService = Proxy.newProxyInstance(
                    powerInterface.getClassLoader(), new Class<?>[] {powerInterface},
                    new DefaultServiceHandler());
            Object thermalService = Proxy.newProxyInstance(
                    thermalInterface.getClassLoader(), new Class<?>[] {thermalInterface},
                    new ThermalServiceHandler());
            Class<?> type = Class.forName("android.os.PowerManager");
            Constructor<?> constructor = type.getDeclaredConstructor(
                    Context.class, powerInterface, thermalInterface, Handler.class);
            constructor.setAccessible(true);
            return constructor.newInstance(
                    this, powerService, thermalService, new Handler(Looper.getMainLooper()));
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not construct android.os.PowerManager", error);
        }
    }

    private UserManager constructUserManager() {
        try {
            Class<?> serviceManager = Class.forName("android.os.ServiceManager");
            Method getService = serviceManager.getDeclaredMethod("getService", String.class);
            getService.setAccessible(true);
            IBinder binder = (IBinder) getService.invoke(null, USER_SERVICE);
            if (binder == null) {
                throw new IllegalStateException("IUserManager is not installed");
            }
            Class<?> interfaceClass = Class.forName("android.os.IUserManager");
            Class<?> stubClass = Class.forName("android.os.IUserManager$Stub");
            Method asInterface = stubClass.getMethod("asInterface", IBinder.class);
            Object service = asInterface.invoke(null, binder);
            Constructor<UserManager> constructor = UserManager.class.getDeclaredConstructor(
                    Context.class, interfaceClass);
            constructor.setAccessible(true);
            return constructor.newInstance(this, service);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not construct android.os.UserManager", error);
        }
    }

    private Object constructDevicePolicyManager() {
        try {
            Class<?> serviceManager = Class.forName("android.os.ServiceManager");
            Method getService = serviceManager.getDeclaredMethod("getService", String.class);
            getService.setAccessible(true);
            IBinder binder = (IBinder) getService.invoke(null, DEVICE_POLICY_SERVICE);
            if (binder == null) {
                throw new IllegalStateException("IDevicePolicyManager is not installed");
            }
            Class<?> interfaceClass = Class.forName(
                    "android.app.admin.IDevicePolicyManager");
            Class<?> stubClass = Class.forName(
                    "android.app.admin.IDevicePolicyManager$Stub");
            Method asInterface = stubClass.getMethod("asInterface", IBinder.class);
            Object service = asInterface.invoke(null, binder);
            Class<?> managerClass = Class.forName("android.app.admin.DevicePolicyManager");
            Constructor<?> constructor = managerClass.getDeclaredConstructor(
                    Context.class, interfaceClass);
            constructor.setAccessible(true);
            return constructor.newInstance(this, service);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException(
                    "Could not construct android.app.admin.DevicePolicyManager", error);
        }
    }

    private Object constructUsageStatsManager() {
        try {
            Class<?> serviceManager = Class.forName("android.os.ServiceManager");
            Method getService = serviceManager.getDeclaredMethod("getService", String.class);
            getService.setAccessible(true);
            IBinder binder = (IBinder) getService.invoke(null, USAGE_STATS_SERVICE);
            if (binder == null) {
                throw new IllegalStateException("IUsageStatsManager is not installed");
            }
            Class<?> interfaceClass = Class.forName(
                    "android.app.usage.IUsageStatsManager");
            Class<?> stubClass = Class.forName(
                    "android.app.usage.IUsageStatsManager$Stub");
            Method asInterface = stubClass.getMethod("asInterface", IBinder.class);
            Object service = asInterface.invoke(null, binder);
            Class<?> managerClass = Class.forName(
                    "android.app.usage.UsageStatsManager");
            Constructor<?> constructor = managerClass.getDeclaredConstructor(
                    Context.class, interfaceClass);
            constructor.setAccessible(true);
            return constructor.newInstance(this, service);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException(
                    "Could not construct android.app.usage.UsageStatsManager", error);
        }
    }

    private Object constructJobScheduler() {
        try {
            Class<?> serviceManager = Class.forName("android.os.ServiceManager");
            Method getService = serviceManager.getDeclaredMethod("getService", String.class);
            getService.setAccessible(true);
            IBinder binder = (IBinder) getService.invoke(null, JOB_SCHEDULER_SERVICE);
            if (binder == null) throw new IllegalStateException("IJobScheduler is not installed");
            Class<?> interfaceClass = Class.forName("android.app.job.IJobScheduler");
            Class<?> stubClass = Class.forName("android.app.job.IJobScheduler$Stub");
            Method asInterface = stubClass.getMethod("asInterface", IBinder.class);
            Object service = asInterface.invoke(null, binder);
            Class<?> managerClass = Class.forName("android.app.JobSchedulerImpl");
            Constructor<?> constructor = managerClass.getDeclaredConstructor(Context.class,
                    interfaceClass);
            constructor.setAccessible(true);
            return constructor.newInstance(this, service);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not construct android.app.JobScheduler", error);
        }
    }

    private Object constructAppOpsManager() {
        try {
            Class<?> interfaceType = Class.forName(
                    "com.android.internal.app.IAppOpsService");
            Object service = Proxy.newProxyInstance(interfaceType.getClassLoader(),
                    new Class<?>[] {interfaceType}, new DefaultServiceHandler());
            Class<?> type = Class.forName("android.app.AppOpsManager");
            Field staticService = type.getDeclaredField("sService");
            staticService.setAccessible(true);
            staticService.set(null, service);
            Constructor<?> constructor = type.getDeclaredConstructor(
                    Context.class, interfaceType);
            constructor.setAccessible(true);
            return constructor.newInstance(this, service);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not construct app-ops manager", error);
        }
    }

}
