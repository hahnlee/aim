package dev.darwinart.runtime.job;

import android.app.IServiceConnection;
import android.app.job.JobInfo;
import android.app.job.JobParameters;
import android.content.ComponentName;
import android.content.Intent;
import android.net.Network;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import dev.darwinart.runtime.am.SystemServiceBindings;
import java.lang.reflect.Constructor;

/** One authenticated binding and callback incarnation of an admitted job. */
final class JobServiceContext {
    interface Listener {
        void onStartAcknowledged(JobServiceContext context, boolean ongoing);
        void onFinished(JobServiceContext context, boolean reschedule);
        void onExecutionFailure(JobServiceContext context);
    }

    private static final String SERVICE_DESCRIPTOR = "android.app.job.IJobService";
    private static final String CALLBACK_DESCRIPTOR = "android.app.job.IJobCallback";
    private static final int TRANSACTION_START_JOB = IBinder.FIRST_CALL_TRANSACTION;
    private static final int TRANSACTION_STOP_JOB = IBinder.FIRST_CALL_TRANSACTION + 1;
    private static final int TRANSACTION_ACKNOWLEDGE_START = IBinder.FIRST_CALL_TRANSACTION + 2;
    private static final int TRANSACTION_ACKNOWLEDGE_STOP = IBinder.FIRST_CALL_TRANSACTION + 3;
    private static final int TRANSACTION_JOB_FINISHED = IBinder.FIRST_CALL_TRANSACTION + 6;

    final JobRecord record;
    private final SystemServiceBindings bindings;
    private final ApplicationProcessRegistry processes;
    private final Listener listener;
    private final Network network;
    private final Callback callback = new Callback();
    private final IServiceConnection connection = new ServiceConnection();
    private final class ServiceConnection extends Binder implements IServiceConnection {
        ServiceConnection() {
            attachInterface(this, "android.app.IServiceConnection");
        }

        @Override
        public void connected(ComponentName name, IBinder service, boolean dead)
                throws RemoteException {
            if (dead || service == null || !record.job.getService().equals(name)) {
                listener.onExecutionFailure(JobServiceContext.this);
                return;
            }
            serviceBinder = service;
            try {
                service.linkToDeath(() -> listener.onExecutionFailure(JobServiceContext.this), 0);
                send(service, TRANSACTION_START_JOB, parameters());
            } catch (RemoteException error) {
                listener.onExecutionFailure(JobServiceContext.this);
            }
        }

        @Override
        public IBinder asBinder() {
            return this;
        }
    }
    private IBinder serviceBinder;
    private boolean unbound;
    private boolean startAcknowledged;

    JobServiceContext(JobRecord value, SystemServiceBindings serviceBindings,
            ApplicationProcessRegistry processRegistry, Network activeNetwork, Listener owner) {
        record = value;
        bindings = serviceBindings;
        processes = processRegistry;
        network = activeNetwork;
        listener = owner;
    }

    void start() {
        try {
            Intent intent = new Intent().setComponent(record.job.getService());
            if (bindings.bindService(intent, connection) != 1) {
                listener.onExecutionFailure(this);
            }
        } catch (RemoteException | RuntimeException error) {
            listener.onExecutionFailure(this);
        }
    }

    void stop() {
        IBinder service = serviceBinder;
        if (service != null) {
            try {
                send(service, TRANSACTION_STOP_JOB, parameters());
            } catch (RemoteException ignored) {
                // The state owner has already removed the canceled job.
            }
        }
        unbind();
    }

    void timeout() {
        if (!startAcknowledged) listener.onExecutionFailure(this);
    }

    void unbind() {
        if (unbound) return;
        unbound = true;
        try {
            bindings.unbindService(connection);
        } catch (RemoteException ignored) {
            // Binder death is already represented by this execution ending.
        }
    }

    private void requireOwner() {
        ApplicationProcessRegistry.AttachedApplication caller =
                processes.requireAttachedProcess(Binder.getCallingPid());
        ComponentName component = record.job.getService();
        if (caller.uid != record.uid || !record.packageName.equals(caller.packageName)
                || !component.getPackageName().equals(caller.packageName)) {
            throw new SecurityException("Job callback came from another application");
        }
    }

    private JobParameters parameters() {
        JobInfo job = record.job;
        try {
            Constructor<JobParameters> constructor = JobParameters.class.getDeclaredConstructor(
                    IBinder.class, String.class, int.class,
                    android.os.PersistableBundle.class, Bundle.class,
                    android.content.ClipData.class, int.class,
                    boolean.class, boolean.class, boolean.class,
                    android.net.Uri[].class, String[].class, Network.class);
            constructor.setAccessible(true);
            return constructor.newInstance(callback, record.namespace, job.getId(), job.getExtras(),
                    job.getTransientExtras(), job.getClipData(), job.getClipGrantFlags(), false,
                    job.isExpedited(), job.isUserInitiated(), null, null, network);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android 16 JobParameters constructor changed", error);
        }
    }

    private static void send(IBinder service, int transaction, JobParameters parameters)
            throws RemoteException {
        Parcel data = Parcel.obtain();
        try {
            data.writeInterfaceToken(SERVICE_DESCRIPTOR);
            data.writeTypedObject(parameters, 0);
            if (!service.transact(transaction, data, null, IBinder.FLAG_ONEWAY)) {
                throw new RemoteException("IJobService transaction rejected");
            }
        } finally {
            data.recycle();
        }
    }

    private final class Callback extends Binder {
        Callback() {
            attachInterface(null, CALLBACK_DESCRIPTOR);
        }

        @Override
        protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
                throws RemoteException {
            if (code == INTERFACE_TRANSACTION) {
                if (reply != null) reply.writeString(CALLBACK_DESCRIPTOR);
                return true;
            }
            if (reply == null || (code != TRANSACTION_ACKNOWLEDGE_START
                    && code != TRANSACTION_ACKNOWLEDGE_STOP
                    && code != TRANSACTION_JOB_FINISHED)) {
                return super.onTransact(code, data, reply, flags);
            }
            data.enforceInterface(CALLBACK_DESCRIPTOR);
            int jobId = data.readInt();
            boolean value = data.readBoolean();
            data.enforceNoDataAvail();
            if (jobId != record.job.getId()) throw new SecurityException("Wrong job callback id");
            requireOwner();
            if (code == TRANSACTION_ACKNOWLEDGE_START) {
                startAcknowledged = true;
                listener.onStartAcknowledged(JobServiceContext.this, value);
            } else if (code == TRANSACTION_JOB_FINISHED) {
                listener.onFinished(JobServiceContext.this, value);
            }
            reply.writeNoException();
            return true;
        }
    }
}
