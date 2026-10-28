package android.os;
public class Handler {
    public Handler() {}
    public Handler(android.os.Looper looper) {}
    public android.os.Looper getLooper() { return null; }
    public static java.util.function.Consumer<Runnable> dispatch = Runnable::run;
    public final boolean post(Runnable action) { dispatch.accept(action); return true; }
}
