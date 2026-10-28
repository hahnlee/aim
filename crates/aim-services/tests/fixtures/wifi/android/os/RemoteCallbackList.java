package android.os;
import java.util.ArrayList;
import java.util.LinkedHashMap;
// Android boundary double: identity, duplicate replacement and binder death.
// The production class links to the original image's RemoteCallbackList.
public class RemoteCallbackList<E extends IInterface> {
    private final class Entry implements IBinder.DeathRecipient {
        final E callback;
        Entry(E callback) { this.callback = callback; }
        public void binderDied() { entries.remove(callback.asBinder()); }
    }
    private final LinkedHashMap<IBinder, Entry> entries = new LinkedHashMap<>();
    private ArrayList<E> broadcast;
    public boolean register(E callback) {
        unregister(callback);
        Entry entry = new Entry(callback);
        try { callback.asBinder().linkToDeath(entry, 0); }
        catch (RemoteException e) { return false; }
        entries.put(callback.asBinder(), entry);
        return true;
    }
    public boolean unregister(E callback) {
        Entry entry = entries.remove(callback.asBinder());
        if (entry == null) return false;
        callback.asBinder().unlinkToDeath(entry, 0);
        return true;
    }
    public int beginBroadcast() {
        if (broadcast != null) throw new AssertionError("nested broadcast");
        broadcast = new ArrayList<>();
        for (Entry entry : entries.values()) broadcast.add(entry.callback);
        return broadcast.size();
    }
    public E getBroadcastItem(int index) { return broadcast.get(index); }
    public void finishBroadcast() { broadcast = null; }
}
