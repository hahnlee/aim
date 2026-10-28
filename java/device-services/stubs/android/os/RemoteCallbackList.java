package android.os;

public class RemoteCallbackList<E extends IInterface> {
    public RemoteCallbackList() {}
    public boolean register(E callback) { return false; }
    public boolean unregister(E callback) { return false; }
    public int beginBroadcast() { return 0; }
    public E getBroadcastItem(int index) { return null; }
    public void finishBroadcast() {}
}
