package dev.aim.server;
import android.app.job.JobInfo;
import android.os.RemoteException;
import java.util.List;
interface ISystemProcess {
    List<JobInfo> getSystemScheduledOwnJobs(String namespace) throws RemoteException;
    int[] getJobPersistStats() throws RemoteException;
}
