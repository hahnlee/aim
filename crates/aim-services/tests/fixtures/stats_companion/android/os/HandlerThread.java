// Host-only Android boundary double; production policy is compiled unchanged.
package android.os;
public class HandlerThread extends Thread {
    public static int active;
    public HandlerThread(String name) { super(name); }
    public HandlerThread(String name,int priority) { super(name); }
    @Override public void start() { active++; }
    public Looper getLooper() { return new Looper(); }
    public boolean quit() { active--; return true; }
    public boolean quitSafely() { return quit(); }
}
