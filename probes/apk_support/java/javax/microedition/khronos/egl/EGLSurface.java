package javax.microedition.khronos.egl;

public final class EGLSurface implements EGL {
    final int kind;
    EGLSurface(int kind) { this.kind = kind; }
}
