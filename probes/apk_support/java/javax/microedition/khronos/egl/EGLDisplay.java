package javax.microedition.khronos.egl;

public final class EGLDisplay implements EGL {
    final int kind;
    EGLDisplay(int kind) { this.kind = kind; }
}
