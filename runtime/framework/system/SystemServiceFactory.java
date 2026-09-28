package dev.aim.runtime.system;

import android.os.Binder;
import android.os.IBinder;
import com.android.server.LocalServices;
import com.android.server.wm.ActivityTaskManagerInternal;
import dev.aim.runtime.admin.DevicePolicyManagerEndpoint;
import dev.aim.runtime.alarm.AlarmManagerEndpoint;
import dev.aim.runtime.am.ActivityManagerEndpoint;
import dev.aim.runtime.am.ApplicationProcessRegistry;
import dev.aim.runtime.audio.AudioServiceEndpoint;
import dev.aim.runtime.camera.CameraServiceEndpoint;
import dev.aim.runtime.content.ClipboardServiceEndpoint;
import dev.aim.runtime.content.ContentServiceEndpoint;
import dev.aim.runtime.connectivity.ConnectivityManagerEndpoint;
import dev.aim.runtime.connectivity.ConnectivityCallbackRegistry;
import dev.aim.runtime.connectivity.ConnectivityServiceState;
import dev.aim.runtime.connectivity.CallerConnectivityPermissionEnforcer;
import dev.aim.runtime.connectivity.NetworkPathProvider;
import dev.aim.runtime.display.DisplayManagerEndpoint;
import dev.aim.runtime.display.TaskDisplayRegistry;
import dev.aim.runtime.wm.ActivityTaskManagerLocal;
import dev.aim.runtime.input.InputManagerEndpoint;
import dev.aim.runtime.inputmethod.InputMethodManagerEndpoint;
import dev.aim.runtime.job.JobSchedulerEndpoint;
import dev.aim.runtime.job.JobSchedulerService;
import dev.aim.runtime.locale.LocaleManagerEndpoint;
import dev.aim.runtime.notification.NotificationManagerEndpoint;
import dev.aim.runtime.am.ApplicationPackages;
import dev.aim.runtime.power.BatteryPropertiesRegistrarEndpoint;
import dev.aim.runtime.power.BatteryService;
import dev.aim.runtime.power.BatteryStatsEndpoint;
import dev.aim.runtime.power.DarwinBatteryStateProvider;
import dev.aim.runtime.power.DarwinPowerStateProvider;
import dev.aim.runtime.power.PowerManagerEndpoint;
import dev.aim.runtime.power.ThermalServiceEndpoint;
import dev.aim.runtime.restrictions.RestrictionsManagerEndpoint;
import dev.aim.runtime.shortcut.ShortcutManagerEndpoint;
import dev.aim.runtime.storage.StorageManagerEndpoint;
import dev.aim.runtime.trust.TrustManagerEndpoint;
import dev.aim.runtime.uimode.UiModeManagerEndpoint;
import dev.aim.runtime.user.UserManagerEndpoint;
import dev.aim.runtime.usage.UsageStatsManagerEndpoint;
import dev.aim.runtime.wm.ActivityTaskManagerEndpoint;
import dev.aim.runtime.wm.DesktopRootEndpoint;
import dev.aim.runtime.wm.DesktopRootGeometryEndpoint;
import dev.aim.runtime.wm.DesktopRootRegistry;
import dev.aim.runtime.wm.DesktopWindowMetadataEndpoint;
import dev.aim.runtime.wm.DesktopWindowMetadataRegistry;
import dev.aim.runtime.wm.TaskGeometryController;
import dev.aim.runtime.wm.WindowManagerEndpoint;
import java.util.HashMap;

/** Production owner for constructing the system process's immutable service table. */
public final class SystemServiceFactory {
    private SystemServiceFactory() {}

    /**
     * The runtime's system service endpoints. PackageManagerService publishes
     * "package" itself when SystemServerBootstrap starts it.
     */
    public static Binder create() {
        HashMap<String, IBinder> services = new HashMap<>();
        ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        DesktopWindowMetadataRegistry windowMetadata = new DesktopWindowMetadataRegistry();
        // Display, WMS and ActivityTask share one per-task geometry owner.
        TaskDisplayRegistry taskDisplays = new TaskDisplayRegistry();
        WindowManagerEndpoint windowManager =
                new WindowManagerEndpoint(processes, windowMetadata, taskDisplays);
        TaskGeometryController taskGeometry =
                TaskGeometryController.create(processes, taskDisplays, windowManager);
        taskGeometry.install();
        ActivityManagerEndpoint activity =
                new ActivityManagerEndpoint(processes, taskGeometry);
        services.put("activity", activity);
        LocalServices.addService(android.app.ActivityManagerInternal.class, activity.localService());
        BatteryService battery =
                new BatteryService(new DarwinBatteryStateProvider(), activity.systemBroadcasts());
        battery.start();
        new dev.aim.runtime.time.HostTimeZoneService(activity.systemBroadcasts()).start();
        services.put("batteryproperties", new BatteryPropertiesRegistrarEndpoint(battery));
        services.put("batterystats", new BatteryStatsEndpoint(battery));
        services.put("activity_task", new ActivityTaskManagerEndpoint(processes));
        LocalServices.addService(ActivityTaskManagerInternal.class, new ActivityTaskManagerLocal());
        services.put("display", new DisplayManagerEndpoint(taskDisplays));
        DesktopRootRegistry desktopRoots = windowManager.createDesktopRootRegistry();
        services.put("window", windowManager);
        services.put("aim.root_geometry", new DesktopRootGeometryEndpoint(taskGeometry));
        services.put("aim.window_metadata",
                new DesktopWindowMetadataEndpoint(processes, windowMetadata));
        services.put("aim.desktop_root", new DesktopRootEndpoint(processes, desktopRoots));
        services.put("user", new UserManagerEndpoint());
        services.put("recovery", new dev.aim.runtime.recovery.RecoverySystemEndpoint());
        services.put("content", new ContentServiceEndpoint());
        services.put("clipboard", new ClipboardServiceEndpoint());
        services.put("notification", new NotificationManagerEndpoint());
        services.put("input_method", new InputMethodManagerEndpoint());
        services.put("input", new InputManagerEndpoint());
        services.put("audio", new AudioServiceEndpoint());
        services.put("media.camera", new CameraServiceEndpoint());
        services.put("alarm", new AlarmManagerEndpoint());
        services.put("shortcut", new ShortcutManagerEndpoint(processes));
        services.put("mount", new StorageManagerEndpoint(processes));
        LocalServices.addService(android.os.storage.StorageManagerInternal.class,
                new dev.aim.runtime.storage.StorageManagerLocal());
        services.put("device_policy", new DevicePolicyManagerEndpoint());
        services.put("power", new PowerManagerEndpoint(new DarwinPowerStateProvider()));
        services.put("thermalservice", new ThermalServiceEndpoint());
        services.put("restrictions", new RestrictionsManagerEndpoint());
        services.put("trust", new TrustManagerEndpoint());
        services.put("uimode", new UiModeManagerEndpoint());
        services.put("locale", new LocaleManagerEndpoint());
        services.put("usagestats", new UsageStatsManagerEndpoint());
        LocalServices.addService(android.app.usage.UsageStatsManagerInternal.class,
                new dev.aim.runtime.usage.UsageStatsManagerLocal());
        NetworkPathProvider networkPath = new NetworkPathProvider();
        ConnectivityServiceState connectivity = new ConnectivityServiceState(networkPath);
        new dev.aim.runtime.connectivity.HostProxyService(
                connectivity, activity.systemBroadcasts()).start();
        JobSchedulerService jobs = new JobSchedulerService(
                ApplicationPackages.INSTANCE, processes,
                activity.systemServiceBindings(), connectivity);
        services.put("jobscheduler", new JobSchedulerEndpoint(processes, jobs));
        services.put("connectivity", new ConnectivityManagerEndpoint(
                new CallerConnectivityPermissionEnforcer(processes), connectivity,
                new ConnectivityCallbackRegistry(connectivity)));
        return new ServiceDirectory(services);
    }
}
