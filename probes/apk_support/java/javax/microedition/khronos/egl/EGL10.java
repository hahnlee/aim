package javax.microedition.khronos.egl;

public interface EGL10 extends EGL {
    EGLDisplay EGL_DEFAULT_DISPLAY = new EGLDisplay(0);
    EGLContext EGL_NO_CONTEXT = new EGLContext(-1);
    EGLDisplay EGL_NO_DISPLAY = new EGLDisplay(-1);
    EGLSurface EGL_NO_SURFACE = new EGLSurface(-1);
    int EGL_FALSE = 0;
    int EGL_TRUE = 1;
    int EGL_NONE = 0x3038;
    int EGL_RED_SIZE = 0x3024;
    int EGL_GREEN_SIZE = 0x3023;
    int EGL_BLUE_SIZE = 0x3022;
    int EGL_ALPHA_SIZE = 0x3021;
    int EGL_DEPTH_SIZE = 0x3025;
    int EGL_STENCIL_SIZE = 0x3026;
    int EGL_RENDERABLE_TYPE = 0x3040;
    int EGL_CONTEXT_CLIENT_VERSION = 0x3098;

    EGLDisplay eglGetDisplay(Object displayId);
    boolean eglInitialize(EGLDisplay display, int[] majorMinor);
    boolean eglChooseConfig(EGLDisplay display, int[] attributes, EGLConfig[] configs,
                            int configSize, int[] numConfig);
    EGLContext eglCreateContext(EGLDisplay display, EGLConfig config, EGLContext share,
                                int[] attributes);
    EGLSurface eglCreatePbufferSurface(EGLDisplay display, EGLConfig config, int[] attributes);
    EGLSurface eglCreateWindowSurface(EGLDisplay display, EGLConfig config, Object nativeWindow,
                                      int[] attributes);
    boolean eglMakeCurrent(EGLDisplay display, EGLSurface draw, EGLSurface read,
                           EGLContext context);
    boolean eglDestroySurface(EGLDisplay display, EGLSurface surface);
    boolean eglDestroyContext(EGLDisplay display, EGLContext context);
    boolean eglTerminate(EGLDisplay display);
    boolean eglSwapBuffers(EGLDisplay display, EGLSurface surface);
    int eglGetError();
}

final class DarwinEGL10 implements EGL10 {
    private int error;
    public EGLDisplay eglGetDisplay(Object id) { return EGL_DEFAULT_DISPLAY; }
    public boolean eglInitialize(EGLDisplay d, int[] v) {
        if (v != null && v.length >= 2) { v[0] = 1; v[1] = 5; }
        return true;
    }
    public boolean eglChooseConfig(EGLDisplay d, int[] a, EGLConfig[] c, int n, int[] out) {
        if (c != null && c.length != 0 && n != 0) c[0] = new EGLConfig(1);
        if (out != null && out.length != 0) out[0] = 1;
        return true;
    }
    public EGLContext eglCreateContext(EGLDisplay d, EGLConfig c, EGLContext s, int[] a) {
        return new EGLContext(2);
    }
    public EGLSurface eglCreatePbufferSurface(EGLDisplay d, EGLConfig c, int[] a) {
        return new EGLSurface(3);
    }
    public EGLSurface eglCreateWindowSurface(EGLDisplay d, EGLConfig c, Object w, int[] a) {
        return new EGLSurface(4);
    }
    public boolean eglMakeCurrent(EGLDisplay d, EGLSurface draw, EGLSurface read, EGLContext c) {
        return true;
    }
    public boolean eglDestroySurface(EGLDisplay d, EGLSurface s) { return true; }
    public boolean eglDestroyContext(EGLDisplay d, EGLContext c) { return true; }
    public boolean eglTerminate(EGLDisplay d) { return true; }
    public boolean eglSwapBuffers(EGLDisplay d, EGLSurface s) { return true; }
    public int eglGetError() { int e = error; error = 0x3000; return e == 0 ? 0x3000 : e; }
}
