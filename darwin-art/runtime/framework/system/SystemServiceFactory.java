package dev.darwinart.runtime.system;

import android.os.Binder;
import android.os.IBinder;
import dev.darwinart.runtime.admin.DevicePolicyManagerEndpoint;
import dev.darwinart.runtime.alarm.AlarmManagerEndpoint;
import dev.darwinart.runtime.am.ActivityManagerEndpoint;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import dev.darwinart.runtime.appops.AppOpsServiceEndpoint;
import dev.darwinart.runtime.audio.AudioServiceEndpoint;
import dev.darwinart.runtime.camera.CameraServiceEndpoint;
import dev.darwinart.runtime.content.ClipboardServiceEndpoint;
import dev.darwinart.runtime.content.ContentServiceEndpoint;
import dev.darwinart.runtime.connectivity.ConnectivityManagerEndpoint;
import dev.darwinart.runtime.connectivity.ConnectivityCallbackRegistry;
import dev.darwinart.runtime.connectivity.ConnectivityServiceState;
import dev.darwinart.runtime.connectivity.InstalledConnectivityPermissionEnforcer;
import dev.darwinart.runtime.connectivity.NetworkPathProvider;
import dev.darwinart.runtime.display.DisplayManagerEndpoint;
import dev.darwinart.runtime.display.TaskDisplayRegistry;
import dev.darwinart.runtime.input.InputManagerEndpoint;
import dev.darwinart.runtime.inputmethod.InputMethodManagerEndpoint;
import dev.darwinart.runtime.job.JobSchedulerEndpoint;
import dev.darwinart.runtime.job.JobSchedulerService;
import dev.darwinart.runtime.locale.LocaleManagerEndpoint;
import dev.darwinart.runtime.notification.NotificationManagerEndpoint;
import dev.darwinart.runtime.pm.InstalledPackageInfos;
import dev.darwinart.runtime.power.BatteryPropertiesRegistrarEndpoint;
import dev.darwinart.runtime.power.BatteryService;
import dev.darwinart.runtime.power.BatteryStatsEndpoint;
import dev.darwinart.runtime.power.DarwinBatteryStateProvider;
import dev.darwinart.runtime.power.DarwinPowerStateProvider;
import dev.darwinart.runtime.power.PowerManagerEndpoint;
import dev.darwinart.runtime.power.ThermalServiceEndpoint;
import dev.darwinart.runtime.pm.PackageManagerEndpoint;
import dev.darwinart.runtime.pm.PackageRecords;
import dev.darwinart.runtime.restrictions.RestrictionsManagerEndpoint;
import dev.darwinart.runtime.shortcut.ShortcutManagerEndpoint;
import dev.darwinart.runtime.storage.StorageManagerEndpoint;
import dev.darwinart.runtime.trust.TrustManagerEndpoint;
import dev.darwinart.runtime.uimode.UiModeManagerEndpoint;
import dev.darwinart.runtime.user.UserManagerEndpoint;
import dev.darwinart.runtime.usage.UsageStatsManagerEndpoint;
import dev.darwinart.runtime.wm.ActivityTaskManagerEndpoint;
import dev.darwinart.runtime.wm.DesktopRootEndpoint;
import dev.darwinart.runtime.wm.DesktopRootGeometryEndpoint;
import dev.darwinart.runtime.wm.DesktopRootRegistry;
import dev.darwinart.runtime.wm.DesktopWindowMetadataEndpoint;
import dev.darwinart.runtime.wm.DesktopWindowMetadataRegistry;
import dev.darwinart.runtime.wm.TaskGeometryController;
import dev.darwinart.runtime.wm.WindowManagerEndpoint;
import java.util.HashMap;

/** Production owner for constructing the system process's immutable service table. */
public final class SystemServiceFactory {
    private SystemServiceFactory() {}

    public static <T extends Binder & PackageRecords.Source> Binder create(T packages) {
        if (packages == null) throw new NullPointerException("packages");
        HashMap<String, IBinder> services = new HashMap<>();
        ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        DesktopWindowMetadataRegistry windowMetadata = new DesktopWindowMetadataRegistry();
        services.put("darwin.package_registry", packages);
        services.put("package", new PackageManagerEndpoint(packages, processes::isCallerSameApp));
        // Display, WMS and ActivityTask share one per-task geometry owner.
        TaskDisplayRegistry taskDisplays = new TaskDisplayRegistry();
        WindowManagerEndpoint windowManager =
                new WindowManagerEndpoint(processes, windowMetadata, taskDisplays);
        TaskGeometryController taskGeometry =
                TaskGeometryController.create(processes, taskDisplays, windowManager);
        taskGeometry.install();
        ActivityManagerEndpoint activity =
                new ActivityManagerEndpoint(packages, processes, taskGeometry);
        services.put("activity", activity);
        BatteryService battery =
                new BatteryService(new DarwinBatteryStateProvider(), activity.systemBroadcasts());
        battery.start();
        services.put("batteryproperties", new BatteryPropertiesRegistrarEndpoint(battery));
        services.put("batterystats", new BatteryStatsEndpoint(battery));
        services.put("activity_task", new ActivityTaskManagerEndpoint(packages, processes));
        services.put("display", new DisplayManagerEndpoint(taskDisplays));
        DesktopRootRegistry desktopRoots = windowManager.createDesktopRootRegistry();
        services.put("window", windowManager);
        services.put("darwin.root_geometry", new DesktopRootGeometryEndpoint(taskGeometry));
        services.put("darwin.window_metadata",
                new DesktopWindowMetadataEndpoint(processes, windowMetadata));
        services.put("darwin.desktop_root", new DesktopRootEndpoint(processes, desktopRoots));
        services.put("user", new UserManagerEndpoint());
        services.put("content", new ContentServiceEndpoint());
        services.put("clipboard", new ClipboardServiceEndpoint());
        services.put("notification", new NotificationManagerEndpoint());
        services.put("input_method", new InputMethodManagerEndpoint());
        services.put("input", new InputManagerEndpoint());
        services.put("audio", new AudioServiceEndpoint());
        services.put("media.camera", new CameraServiceEndpoint());
        services.put("alarm", new AlarmManagerEndpoint());
        services.put("appops", new AppOpsServiceEndpoint());
        services.put("shortcut", new ShortcutManagerEndpoint(processes));
        services.put("mount", new StorageManagerEndpoint(processes));
        services.put("device_policy", new DevicePolicyManagerEndpoint());
        services.put("power", new PowerManagerEndpoint(new DarwinPowerStateProvider()));
        services.put("thermalservice", new ThermalServiceEndpoint());
        services.put("restrictions", new RestrictionsManagerEndpoint());
        services.put("trust", new TrustManagerEndpoint());
        services.put("uimode", new UiModeManagerEndpoint());
        services.put("locale", new LocaleManagerEndpoint());
        services.put("usagestats", new UsageStatsManagerEndpoint());
        NetworkPathProvider networkPath = new NetworkPathProvider();
        ConnectivityServiceState connectivity = new ConnectivityServiceState(networkPath);
        JobSchedulerService jobs = new JobSchedulerService(
                InstalledPackageInfos.serviceResolver(packages), processes,
                activity.systemServiceBindings(), connectivity);
        services.put("jobscheduler", new JobSchedulerEndpoint(processes, jobs));
        services.put("connectivity", new ConnectivityManagerEndpoint(
                new InstalledConnectivityPermissionEnforcer(packages, processes), connectivity,
                new ConnectivityCallbackRegistry(connectivity)));
        return new ServiceDirectory(services);
    }
}
