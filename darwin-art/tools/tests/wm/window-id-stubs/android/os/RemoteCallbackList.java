package android.os;

import java.util.ArrayList;

/** Test stub: registration order broadcast, no death tracking. */
public class RemoteCallbackList<E> {
    private final ArrayList<E> callbacks = new ArrayList<>();
    private ArrayList<E> broadcast;

    public synchronized boolean register(E callback) { return callbacks.add(callback); }
    public synchronized boolean unregister(E callback) { return callbacks.remove(callback); }
    public synchronized void kill() { callbacks.clear(); }
    public synchronized int beginBroadcast() {
        broadcast = new ArrayList<>(callbacks);
        return broadcast.size();
    }
    public E getBroadcastItem(int index) { return broadcast.get(index); }
    public void finishBroadcast() { broadcast = null; }
}
