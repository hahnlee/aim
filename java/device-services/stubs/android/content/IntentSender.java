// Compile-only original image API.
package android.content;
public class IntentSender implements android.os.Parcelable {
    public IntentSender(IIntentSender sender) { throw new RuntimeException("stub"); }
    public IntentSender(android.os.IBinder binder) { throw new RuntimeException("stub"); }
    public static final android.os.Parcelable.Creator<IntentSender> CREATOR = null;
    public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public boolean isImmutable() { throw new RuntimeException("stub"); }
    public void sendIntent(Context context, int code, Intent intent, String permission,
            android.os.Bundle options, java.util.concurrent.Executor executor,
            OnFinished finished) throws SendIntentException { throw new RuntimeException("stub"); }
    public interface OnFinished {}
    public static class SendIntentException extends android.util.AndroidException {
        public SendIntentException() { throw new RuntimeException("stub"); }
    }
}
