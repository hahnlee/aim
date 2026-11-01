package android.app.job;
public class JobInfo implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<JobInfo> CREATOR = null;
    public int describeContents() { throw new RuntimeException("stub"); }
    public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    public static final class Builder {
        public Builder(int id, android.content.ComponentName component) { throw new RuntimeException("stub"); }
        public Builder setRequiresDeviceIdle(boolean required) { throw new RuntimeException("stub"); }
        public JobInfo build() { throw new RuntimeException("stub"); }
    }
 private JobInfo(Builder builder){throw new RuntimeException("stub");}
}
