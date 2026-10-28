package android.os;
public record HandlerExecutor(Handler handler) implements java.util.concurrent.Executor {
    public void execute(Runnable action) { handler.post(action); }
}
