package android.app.job;
public abstract class JobScheduler {
    public abstract int schedule(JobInfo job);
    public abstract void cancel(int id);
}
