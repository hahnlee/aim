package dev.aim.server;

import android.content.Context;
import android.os.Binder;
import android.os.IBinder;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.SystemProperties;
import android.util.Slog;

import com.android.internal.os.ApplicationSharedMemory;
import com.android.server.SystemService;

import java.io.IOException;

/**
 * The device's own system service, which SystemServer starts from
 * {@code config_deviceSpecificSystemServices} (the framework overlay of
 * java/framework-overlay), after the system services are ready. It
 * publishes nothing: at its first boot phase it hands the native service
 * host (ADR 0013) the bridge to system_server's internal state
 * (docs/system-services.md, "The system_server bridge").
 * In the lightweight shell it also starts the window shell (WindowShell,
 * docs/task-organizer.md), which aim-windows finds as aim.window_shell.
 */
public final class DeviceServices extends SystemService {
    private static final String TAG = "AimDeviceServices";

    /** The native service host's name in servicemanager. */
    private static final String SERVICE_HOST = "aim.service_host";

    private final LocationBridge mLocation;
    private final UiModeBridge mUiMode;
    private final LocaleBridge mLocale = new LocaleBridge();

    public DeviceServices(Context context) {
        super(context);
        mLocation = new LocationBridge(context);
        mUiMode = new UiModeBridge(context);
    }

    @Override
    public void onStart() {
        mLocation.publish();
    }

    @Override
    public void onBootPhase(int phase) {
        if (phase == PHASE_DEVICE_SPECIFIC_SERVICES_READY) {
            // Before the home activity starts: every task is organized.
            if (SystemProperties.getBoolean(WindowShell.PROPERTY, false)) {
                WindowShell.start();
            }
            attachBridge();
        } else if (phase == PHASE_THIRD_PARTY_APPS_CAN_START) {
            mLocation.onThirdPartyAppsCanStart();
            mLocale.onThirdPartyAppsCanStart();
        }
    }

    @Override
    public void onUserStarting(TargetUser user) {
        mLocation.onUserStarting(user.getUserIdentifier());
    }

    @Override
    public void onUserSwitching(TargetUser from, TargetUser to) {
        mLocation.onUserSwitching(from.getUserIdentifier(), to.getUserIdentifier());
        mUiMode.onUserSwitching(from.getUserIdentifier(), to.getUserIdentifier());
    }

    @Override
    public void onUserStopped(TargetUser user) {
        mLocation.onUserStopped(user.getUserIdentifier());
    }

    private void attachBridge() {
        IBinder host = ServiceManager.checkService(SERVICE_HOST);
        if (host == null) {
            Slog.i(TAG, "no native service host");
            return;
        }
        try {
            IServiceHost service = IServiceHost.Stub.asInterface(host);
            service.attachBridge(new Bridge(getContext(), service, mLocation, mUiMode, mLocale));
            Slog.i(TAG, "bridge attached to the native service host");
        } catch (RemoteException e) {
            Slog.w(TAG, "cannot attach the bridge", e);
        }
    }

    private static final class Bridge extends IBridge.Stub {
        private final Context context;
        private final IServiceHost host;
        private final LocationBridge location;
        private final UiModeBridge uiMode;
        private final LocaleBridge locale;

        Bridge(Context context, IServiceHost host, LocationBridge location, UiModeBridge uiMode,
                LocaleBridge locale) {
            this.context = context;
            this.host = host;
            this.location = location;
            this.uiMode = uiMode;
            this.locale = locale;
        }

        @Override
        public ParcelFileDescriptor getApplicationSharedMemory() {
            enforceSystemUid();
            try {
                return new ParcelFileDescriptor(
                        ApplicationSharedMemory.getInstance().getReadOnlyFileDescriptor());
            } catch (IOException e) {
                throw new IllegalStateException(e);
            }
        }

        @Override
        public void interceptNotificationPermissionRequests() {
            enforceSystemUid();
            NotificationPermissionInterceptor.register(context, host);
        }

        @Override
        public ILocationBridge getLocationBridge(ILocationHost host) {
            enforceSystemUid();
            return location.attach(host);
        }

        @Override
        public IUiModeBridge getUiModeBridge(IUiModeHost host) {
            enforceSystemUid();
            return uiMode.attach(host);
        }

        @Override
        public void updateLocales(String languageTags) {
            enforceSystemUid();
            locale.update(languageTags);
        }

        private static void enforceSystemUid() {
            if (Binder.getCallingUid() != Process.SYSTEM_UID) {
                throw new SecurityException("the bridge serves the system uid only");
            }
        }
    }
}
