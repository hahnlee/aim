package android.app;
public final class ActivityThread {
    private static final ActivityThread thread = new ActivityThread();
    private final ContextImpl context = new ContextImpl();
    public static ActivityThread currentActivityThread() { return thread; }
    public ContextImpl getSystemContext() { return context; }
}
