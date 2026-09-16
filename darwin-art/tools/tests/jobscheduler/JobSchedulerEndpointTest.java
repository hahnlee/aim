package dev.darwinart.runtime.job;

import android.app.job.JobInfo;
import android.content.ComponentName;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.content.pm.ParceledListSlice;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/** Focused wire and ownership test for the Android 16 JobScheduler endpoint. */
public final class JobSchedulerEndpointTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static ApplicationProcessRegistry processes() {
        ApplicationProcessRegistry result = new ApplicationProcessRegistry();
        result.beginAttachment(42, 10042, new Binder(), 1L);
        result.identify(42, "org.example.app");
        result.finishAttachment(42, 1L);
        Binder.setCallingPid(42);
        return result;
    }

    private static JobInfo job(String packageName) {
        return new JobInfo(new ComponentName(packageName));
    }

    private static Parcel scheduleRequest(JobInfo value) {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(JobSchedulerEndpoint.DESCRIPTOR);
        data.writeString(null); // default namespace
        data.writeTypedObject(value, 0);
        return data;
    }

    private static void testScheduleIsDelegatedAndQueryable() throws Exception {
        ApplicationProcessRegistry processes = processes();
        JobSchedulerEndpoint endpoint = new JobSchedulerEndpoint(processes,
                new JobSchedulerService());
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(JobSchedulerEndpoint.TRANSACTION_SCHEDULE,
                scheduleRequest(job("org.example.app")), reply, 0),
                "schedule transaction rejected");
        check(reply.hasNoException(), "schedule omitted writeNoException");
        check(reply.readInt() == JobSchedulerEndpoint.RESULT_SUCCESS,
                "admitted execution did not report RESULT_SUCCESS");
        check(reply.dataAvail() == 0, "schedule reply contained trailing data");
    }

    private static void testOwnershipIsEnforced() throws Exception {
        JobSchedulerEndpoint endpoint = new JobSchedulerEndpoint(processes(),
                new JobSchedulerService());
        try {
            endpoint.onTransact(JobSchedulerEndpoint.TRANSACTION_SCHEDULE,
                    scheduleRequest(job("org.other.app")), Parcel.obtain(), 0);
            throw new AssertionError("foreign job service was accepted");
        } catch (SecurityException expected) {}

        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(JobSchedulerEndpoint.DESCRIPTOR);
        data.writeString(null);
        data.writeTypedObject(job("org.example.app"), 0);
        data.writeString("org.other.app");
        data.writeInt(0);
        data.writeString("upload");
        try {
            endpoint.onTransact(JobSchedulerEndpoint.TRANSACTION_SCHEDULE_AS_PACKAGE,
                    data, Parcel.obtain(), 0);
            throw new AssertionError("foreign scheduleAsPackage was accepted");
        } catch (SecurityException expected) {}
    }

    private static void testEmptyQueryUsesFrameworkParcelable() throws Exception {
        JobSchedulerEndpoint endpoint = new JobSchedulerEndpoint(processes(),
                new JobSchedulerService());
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(JobSchedulerEndpoint.DESCRIPTOR);
        data.writeString(null);
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(JobSchedulerEndpoint.TRANSACTION_GET_ALL_PENDING_JOBS_IN_NAMESPACE,
                data, reply, 0), "namespace query rejected");
        check(reply.hasNoException(), "query omitted writeNoException");
        check(reply.readTypedObject(ParceledListSlice.CREATOR) == ParceledListSlice.emptyList(),
                "query did not return framework empty slice");
        check(reply.dataAvail() == 0, "query reply contained trailing data");
    }

    private static void testDescriptorAndWireValidation() throws Exception {
        JobSchedulerEndpoint endpoint = new JobSchedulerEndpoint(processes(),
                new JobSchedulerService());
        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, Parcel.obtain(), descriptorReply, 0),
                "descriptor transaction rejected");
        check(JobSchedulerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "descriptor changed");

        Parcel trailing = scheduleRequest(job("org.example.app"));
        trailing.writeInt(7);
        try {
            endpoint.onTransact(JobSchedulerEndpoint.TRANSACTION_SCHEDULE,
                    trailing, Parcel.obtain(), 0);
            throw new AssertionError("trailing schedule data was accepted");
        } catch (IllegalStateException expected) {}

        check(!endpoint.onTransact(JobSchedulerEndpoint.TRANSACTION_NOTE_PENDING_USER_REQUESTED_APP_STOP + 1,
                scheduleRequest(job("org.example.app")), Parcel.obtain(), 0),
                "unsupported transaction was accepted");
    }

    public static void main(String[] args) throws Exception {
        check(JobSchedulerEndpoint.TRANSACTION_SCHEDULE == 1, "schedule transaction drifted");
        check(JobSchedulerEndpoint.TRANSACTION_SCHEDULE_AS_PACKAGE == 3,
                "scheduleAsPackage transaction drifted");
        check(JobSchedulerEndpoint.TRANSACTION_GET_ALL_PENDING_JOBS == 7,
                "getAllPendingJobs transaction drifted");
        check(JobSchedulerEndpoint.TRANSACTION_NOTE_PENDING_USER_REQUESTED_APP_STOP == 19,
                "Android 16 transaction count drifted");
        testScheduleIsDelegatedAndQueryable();
        testOwnershipIsEnforced();
        testEmptyQueryUsesFrameworkParcelable();
        testDescriptorAndWireValidation();
        System.out.println("job-scheduler-contract: PASS");
    }
}
