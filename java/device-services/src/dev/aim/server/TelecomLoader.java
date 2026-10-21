package dev.aim.server;

import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.IBinder;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.util.Slog;

import com.android.internal.R;
import com.android.internal.telecom.IInternalServiceRetriever;
import com.android.internal.telecom.ITelecomLoader;
import com.android.internal.telecom.ITelecomService;
import com.android.internal.telephony.SmsApplication;

/**
 * TelecomLoaderService (`android-16.0.0_r1`) in the app host's system
 * process: at PHASE_ACTIVITY_MANAGER_READY it binds Telecom's
 * TelecomService, which runs in this process (`android:process="system"`),
 * and, on this main thread as the binding's connection is told, has it
 * create the telecom service with the system's InternalServiceRepository
 * (the app host's, `aim.system_process`), then publishes it as `telecom`;
 * a lost binding binds again. Its default app providers feed only the
 * default permission grant policy, and its role and carrier config
 * listeners only grant the SIM call manager's permissions through it
 * (LegacyPermissionManagerInternal); the app host runs no such policy, so
 * they are not registered.
 */
final class TelecomLoader {
    private static final String TAG = "TelecomLoaderService";

    private static final ComponentName SERVICE_COMPONENT = new ComponentName(
            "com.android.server.telecom",
            "com.android.server.telecom.components.TelecomService");

    private static final String SERVICE_ACTION = "com.android.ITelecomService";

    private final Object mLock = new Object();
    private final Context mContext;
    private TelecomServiceConnection mServiceConnection;
    private IInternalServiceRetriever mServiceRepo;

    TelecomLoader(Context context) {
        mContext = context;
    }

    private class TelecomServiceConnection implements ServiceConnection {
        @Override
        public void onServiceConnected(ComponentName name, IBinder service) {
            // Normally, we would listen for death here, but since telecom runs in the same process
            // as this loader (process="system") that's redundant here.
            try {
                ITelecomLoader telecomLoader = ITelecomLoader.Stub.asInterface(service);
                ITelecomService telecomService = telecomLoader.createTelecomService(mServiceRepo,
                        systemUiPackageName());

                SmsApplication.getDefaultMmsApplication(mContext, false);
                ServiceManager.addService(Context.TELECOM_SERVICE, telecomService.asBinder());
            } catch (RemoteException e) {
                Slog.w(TAG, "Failed linking to death.");
            }
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            connectToTelecom();
        }
    }

    /** `onBootPhase(PHASE_ACTIVITY_MANAGER_READY)`. */
    void onActivityManagerReady() throws RemoteException {
        setupServiceRepository();
        connectToTelecom();
    }

    private void connectToTelecom() {
        synchronized (mLock) {
            if (mServiceConnection != null) {
                mContext.unbindService(mServiceConnection);
                mServiceConnection = null;
            }

            TelecomServiceConnection serviceConnection = new TelecomServiceConnection();
            Intent intent = new Intent(SERVICE_ACTION);
            intent.setComponent(SERVICE_COMPONENT);
            int flags = Context.BIND_IMPORTANT | Context.BIND_FOREGROUND_SERVICE
                    | Context.BIND_AUTO_CREATE;

            // Bind to Telecom and register the service
            if (mContext.bindServiceAsUser(intent, serviceConnection, flags, UserHandle.SYSTEM)) {
                mServiceConnection = serviceConnection;
            }
        }
    }

    /** `setupServiceRepository`, of the app host's DeviceIdleInternal. */
    private void setupServiceRepository() throws RemoteException {
        mServiceRepo = IInternalServiceRetriever.Stub.asInterface(
                ISystemProcess.Stub.asInterface(ServiceManager.getService("aim.system_process"))
                        .getInternalServiceRepository());
    }

    /**
     * `PackageManagerInternal.getSystemUiServiceComponent().getPackageName()`
     * (`config_systemUIServiceComponent`).
     */
    private String systemUiPackageName() {
        return ComponentName.unflattenFromString(mContext.getResources().getString(
                R.string.config_systemUIServiceComponent)).getPackageName();
    }
}
