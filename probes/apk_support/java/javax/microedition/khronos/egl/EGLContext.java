package javax.microedition.khronos.egl;

public final class EGLContext implements EGL {
    private static final EGL10 IMPL = new DarwinEGL10();
    final int kind;
    EGLContext(int kind) { this.kind = kind; }

    public static EGL getEGL() { return IMPL; }
}
