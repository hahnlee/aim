package android.os;
public final class RemoteCallback {
    public interface OnResultListener { void onResult(Bundle result); }
    private final OnResultListener listener;
    public RemoteCallback(OnResultListener listener) { this.listener = listener; }
    public void sendResult(Bundle result) { listener.onResult(result); }
}
