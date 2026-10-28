package android.os;
public class Handler {
    public static java.util.function.Consumer<Runnable> dispatch = Runnable::run;
    public final boolean post(Runnable action) { dispatch.accept(action); return true; }
}
