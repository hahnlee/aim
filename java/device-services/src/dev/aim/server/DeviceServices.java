package dev.aim.server;

import android.content.Context;
import android.os.Binder;
import android.os.IBinder;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
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
 */
public final class DeviceServices extends SystemService {
    private static final String TAG = "AimDeviceServices";

    /** The native service host's name in servicemanager. */
    private static final String SERVICE_HOST = "aim.service_host";

    public DeviceServices(Context context) {
        super(context);
    }

    @Override
    public void onStart() {}

    @Override
    public void onBootPhase(int phase) {
        if (phase == PHASE_DEVICE_SPECIFIC_SERVICES_READY) {
            attachBridge();
        }
    }

    private static void attachBridge() {
        IBinder host = ServiceManager.checkService(SERVICE_HOST);
        if (host == null) {
            Slog.i(TAG, "no native service host");
            return;
        }
        try {
            IServiceHost.Stub.asInterface(host).attachBridge(new Bridge());
            Slog.i(TAG, "bridge attached to the native service host");
        } catch (RemoteException e) {
            Slog.w(TAG, "cannot attach the bridge", e);
        }
    }

    private static final class Bridge extends IBridge.Stub {
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

        private static void enforceSystemUid() {
            if (Binder.getCallingUid() != Process.SYSTEM_UID) {
                throw new SecurityException("the bridge serves the system uid only");
            }
        }
    }
}
