// An NDK program for tests/graphics.rs: graphics buffers through the
// original libnativewindow/libui (allocator and mapper HALs), and GLES
// through the original libEGL loader and the vendor driver.
//
//   gles_triangle          allocate, render, check pixels, native fences
//                          (sync_file), print timings
//   gles_triangle fork     zygote's pattern: get the display, fork, and
//                          compile and draw in the child (twice: a display
//                          only got, and one initialized, before the fork)
//   gles_triangle pending  import fd 3 without a context, notify the host
//                          on fd 4 after observing a zero-timeout wait
//
// Prints one "ok ..." line per check and "timing ..." lines; exits non-zero
// on the first failure.

#define EGL_EGLEXT_PROTOTYPES
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <GLES2/gl2ext.h>
#include <android/hardware_buffer.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/sync_file.h>
#include <media/NdkImageReader.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define W 64
#define H 64

#define CHECK(cond, ...)                   \
    do {                                   \
        if (!(cond)) {                     \
            printf("FAIL: " __VA_ARGS__);  \
            printf("\n");                  \
            exit(1);                       \
        }                                  \
    } while (0)

static double now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1e9 + ts.tv_nsec;
}

static const char* kVertex =
    "#version 300 es\n"
    "in vec2 pos;\n"
    "void main() { gl_Position = vec4(pos, 0.0, 1.0); }\n";
static const char* kFragment =
    "#version 300 es\n"
    "precision mediump float;\n"
    "out vec4 color;\n"
    "void main() { color = vec4(0.0, 1.0, 0.0, 1.0); }\n";

static GLuint shader(GLenum type, const char* src) {
    GLuint s = glCreateShader(type);
    glShaderSource(s, 1, &src, NULL);
    glCompileShader(s);
    GLint ok = 0;
    glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[1024] = "";
        glGetShaderInfoLog(s, sizeof(log), NULL, log);
        CHECK(0, "shader compile: %s", log);
    }
    return s;
}

static GLuint program(void) {
    GLuint p = glCreateProgram();
    glAttachShader(p, shader(GL_VERTEX_SHADER, kVertex));
    glAttachShader(p, shader(GL_FRAGMENT_SHADER, kFragment));
    glBindAttribLocation(p, 0, "pos");
    glLinkProgram(p);
    GLint ok = 0;
    glGetProgramiv(p, GL_LINK_STATUS, &ok);
    CHECK(ok, "program link");
    return p;
}

// A triangle covering the centre of the target, clear color elsewhere.
static void frame(GLuint prog, float r, float g, float b) {
    static const float kTriangle[] = {-0.5f, -0.5f, 0.5f, -0.5f, 0.0f, 0.5f};
    glViewport(0, 0, W, H);
    glClearColor(r, g, b, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    glUseProgram(prog);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, kTriangle);
    glDrawArrays(GL_TRIANGLES, 0, 3);
}

// In a fork child: a context on `dpy` (asked for again and initialized
// here unless `inited`, as an app's RenderThread does, which makes the host
// start ANGLE's display on a thread of its own), a program whose fragment
// shader no cache has seen (a uniform named by the time and pid), and a
// triangle read back. Exits with the result.
static void child_draws(EGLDisplay dpy, int inited) {
    EGLint major = 0, minor = 0;
    if (!inited) {
        CHECK(eglGetDisplay(EGL_DEFAULT_DISPLAY) == dpy, "child eglGetDisplay");
        CHECK(eglInitialize(dpy, &major, &minor), "child eglInitialize %x", eglGetError());
    }
    const EGLint config_attribs[] = {EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8,
                                     EGL_ALPHA_SIZE, 8, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
                                     EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_NONE};
    EGLConfig config;
    EGLint n = 0;
    CHECK(eglChooseConfig(dpy, config_attribs, &config, 1, &n) && n == 1, "child eglChooseConfig %x",
          eglGetError());
    const EGLint context_attribs[] = {EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE};
    EGLContext ctx = eglCreateContext(dpy, config, EGL_NO_CONTEXT, context_attribs);
    CHECK(ctx != EGL_NO_CONTEXT, "child eglCreateContext %x", eglGetError());
    const EGLint pbuffer_attribs[] = {EGL_WIDTH, W, EGL_HEIGHT, H, EGL_NONE};
    EGLSurface pbuffer = eglCreatePbufferSurface(dpy, config, pbuffer_attribs);
    CHECK(eglMakeCurrent(dpy, pbuffer, pbuffer, ctx), "child eglMakeCurrent %x", eglGetError());
    long long nonce = (long long)now_ns();
    char fs[256];
    snprintf(fs, sizeof(fs),
             "#version 300 es\nprecision highp float;\nout vec4 color;\nuniform float u%lld;\n"
             "void main() { color = vec4(0.0, 1.0, 0.0, 1.0) + vec4(u%lld * %d.0); }\n",
             nonce, nonce, getpid());
    GLuint p = glCreateProgram();
    glAttachShader(p, shader(GL_VERTEX_SHADER, kVertex));
    glAttachShader(p, shader(GL_FRAGMENT_SHADER, fs));
    glBindAttribLocation(p, 0, "pos");
    glLinkProgram(p);
    GLint ok = 0;
    glGetProgramiv(p, GL_LINK_STATUS, &ok);
    if (!ok) {
        char log[1024] = "";
        glGetProgramInfoLog(p, sizeof(log), NULL, log);
        CHECK(0, "child program link: %s", log);
    }
    frame(p, 1.0f, 0.0f, 0.0f);
    uint8_t pixels[W * H * 4];
    glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    uint32_t centre = pixels[(H / 2 * W + W / 2) * 4 + 1];
    CHECK(centre == 0xff, "child centre green %02x", centre);
    exit(0);
}

static int fork_and_draw(EGLDisplay dpy, int inited) {
    pid_t pid = fork();
    CHECK(pid >= 0, "fork");
    if (pid == 0) child_draws(dpy, inited);
    int st = 0;
    CHECK(waitpid(pid, &st, 0) == pid, "waitpid");
    return WIFEXITED(st) && WEXITSTATUS(st) == 0;
}

// Zygote preloads the driver (eglGetDisplay) and forks every app without
// exec; the app then compiles its shaders.
static int zygote(void) {
    EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    CHECK(dpy != EGL_NO_DISPLAY, "eglGetDisplay");
    CHECK(fork_and_draw(dpy, 0), "the child of a preloading parent draws");
    printf("ok fork child compiles and draws\n");
    EGLint major = 0, minor = 0;
    CHECK(eglInitialize(dpy, &major, &minor), "eglInitialize %x", eglGetError());
    CHECK(fork_and_draw(dpy, 1), "the child of an initialized parent draws");
    printf("ok fork child of an initialized display draws\n");
    return 0;
}

// Whether a sync_file signals within `ms`.
static int fence_signals(int fd, int ms) {
    struct pollfd p = {.fd = fd, .events = POLLIN};
    return poll(&p, 1, ms) == 1 && p.revents == POLLIN;
}

// A sync_file's status and signal time (SYNC_IOC_FILE_INFO with one fence).
static int fence_info(int fd, uint64_t* timestamp) {
    struct sync_fence_info fence;
    struct sync_file_info info = {.num_fences = 1, .sync_fence_info = (uint64_t)(uintptr_t)&fence};
    CHECK(ioctl(fd, SYNC_IOC_FILE_INFO, &info) == 0, "SYNC_IOC_FILE_INFO %d", errno);
    CHECK(info.num_fences == 1 && fence.status == info.status, "sync_file_info");
    *timestamp = fence.timestamp_ns;
    return info.status;
}

// EGL_ANDROID_native_fence_sync over sync_file: a fence for GL commands,
// waited for, inspected, merged, imported back into EGL, and seen by a
// fork child.
static void native_fences(EGLDisplay dpy, GLuint prog) {
    CHECK(strstr(eglQueryString(dpy, EGL_EXTENSIONS), "EGL_ANDROID_native_fence_sync"),
          "EGL_ANDROID_native_fence_sync");
    PFNEGLDUPNATIVEFENCEFDANDROIDPROC eglDupNativeFenceFDANDROID =
        (PFNEGLDUPNATIVEFENCEFDANDROIDPROC)eglGetProcAddress("eglDupNativeFenceFDANDROID");
    CHECK(eglDupNativeFenceFDANDROID != NULL, "eglDupNativeFenceFDANDROID");
    double t0 = now_ns();
    frame(prog, 0.0f, 0.0f, 1.0f);
    EGLSyncKHR sync = eglCreateSyncKHR(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, NULL);
    CHECK(sync != EGL_NO_SYNC_KHR, "native fence sync %x", eglGetError());
    EGLint value = 0;
    CHECK(eglGetSyncAttribKHR(dpy, sync, EGL_SYNC_TYPE_KHR, &value) &&
              value == EGL_SYNC_NATIVE_FENCE_ANDROID, "sync type %x", value);
    int fd = eglDupNativeFenceFDANDROID(dpy, sync);
    CHECK(fd >= 0, "eglDupNativeFenceFDANDROID %x", eglGetError());
    CHECK(fence_signals(fd, 5000), "the fence signals");
    uint64_t ts = 0;
    CHECK(fence_info(fd, &ts) == 1 && ts >= (uint64_t)t0 && ts <= (uint64_t)now_ns(),
          "signaled status and time");
    CHECK(eglClientWaitSyncKHR(dpy, sync, 0, EGL_FOREVER_KHR) == EGL_CONDITION_SATISFIED,
          "client wait");
    CHECK(eglGetSyncAttribKHR(dpy, sync, EGL_SYNC_STATUS_KHR, &value) && value == EGL_SIGNALED_KHR,
          "sync status");
    char byte;
    CHECK(read(fd, &byte, 1) < 0 && errno == EINVAL, "read of a sync_file");
    eglDestroySyncKHR(dpy, sync);

    // Merge: a fence that has signaled with one that has not yet.
    frame(prog, 1.0f, 0.0f, 0.0f);
    EGLSyncKHR later = eglCreateSyncKHR(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, NULL);
    int later_fd = eglDupNativeFenceFDANDROID(dpy, later);
    struct sync_merge_data merge = {.name = "merged", .fd2 = later_fd};
    CHECK(ioctl(fd, SYNC_IOC_MERGE, &merge) == 0 && merge.fence >= 0, "SYNC_IOC_MERGE %d", errno);
    CHECK(fence_signals(merge.fence, 5000), "the merged fence signals");
    uint64_t later_ts = 0, merged_ts = 0;
    fence_info(later_fd, &later_ts);
    CHECK(fence_info(merge.fence, &merged_ts) == 1 && merged_ts >= later_ts && merged_ts >= ts,
          "merged time %llu", (unsigned long long)merged_ts);
    eglDestroySyncKHR(dpy, later);

    // A fence fd back into EGL (as SurfaceFlinger waits for a buffer):
    // EGL takes the fd over.
    const EGLint attribs[] = {EGL_SYNC_NATIVE_FENCE_FD_ANDROID, dup(merge.fence), EGL_NONE};
    EGLSyncKHR imported = eglCreateSyncKHR(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, attribs);
    CHECK(imported != EGL_NO_SYNC_KHR, "imported fence %x", eglGetError());
    CHECK(eglWaitSyncKHR(dpy, imported, 0) == EGL_TRUE, "server wait %x", eglGetError());
    eglDestroySyncKHR(dpy, imported);

    // A fork child sees the parent's fence, including one signaled after
    // the fork.
    frame(prog, 0.0f, 1.0f, 0.0f);
    EGLSyncKHR pending = eglCreateSyncKHR(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, NULL);
    int pending_fd = eglDupNativeFenceFDANDROID(dpy, pending);
    pid_t pid = fork();
    CHECK(pid >= 0, "fork");
    if (pid == 0) _exit(fence_signals(pending_fd, 5000) && fence_signals(merge.fence, 0) ? 0 : 1);
    int st = 0;
    CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0,
          "fork child waits for the fence");
    eglDestroySyncKHR(dpy, pending);
    close(pending_fd);
    close(merge.fence);
    close(later_fd);
    close(fd);
    CHECK(glGetError() == GL_NO_ERROR, "GL error");
    printf("ok native fences: sync_file wait, info, merge, import, fork\n");

    const int frames = 500;
    t0 = now_ns();
    for (int i = 0; i < frames; i++) {
        frame(prog, 0.0f, 0.0f, 1.0f);
        EGLSyncKHR s = eglCreateSyncKHR(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, NULL);
        int f = eglDupNativeFenceFDANDROID(dpy, s);
        CHECK(fence_signals(f, 5000), "frame fence");
        close(f);
        eglDestroySyncKHR(dpy, s);
    }
    printf("timing triangle frame (clear + draw + native fence + poll) %.1f us\n",
           (now_ns() - t0) / frames / 1e3);
}

// RGBA bytes at (x, y) of a readback with the given row pitch.
static uint32_t at(const uint8_t* p, int pitch, int x, int y) {
    const uint8_t* q = p + y * pitch + x * 4;
    return (uint32_t)q[0] << 24 | (uint32_t)q[1] << 16 | (uint32_t)q[2] << 8 | q[3];
}

static void check_frame(const uint8_t* p, int pitch, uint32_t clear, const char* what) {
    uint32_t centre = at(p, pitch, W / 2, H / 2), corner = at(p, pitch, 1, 1);
    CHECK(centre == 0x00ff00ff, "%s: centre %08x", what, centre);
    CHECK(corner == clear, "%s: corner %08x want %08x", what, corner, clear);
}

static const char* kQuadVertex =
    "#version 300 es\n"
    "uniform vec4 rect;\n"
    "in vec2 pos;\n"
    "out vec2 uv;\n"
    "void main() {\n"
    "    uv = rect.xy + (pos * 0.5 + 0.5) * rect.zw;\n"
    "    gl_Position = vec4(pos, 0.0, 1.0);\n"
    "}\n";
static const char* kExternalFragment =
    "#version 300 es\n"
    "#extension GL_OES_EGL_image_external_essl3 : require\n"
    "precision mediump float;\n"
    "uniform samplerExternalOES tex;\n"
    "in vec2 uv;\n"
    "out vec4 color;\n"
    "void main() { color = texture(tex, uv); }\n";

// `image` (a frame of `frame`) sampled through GL_TEXTURE_EXTERNAL_OES into
// the pbuffer, the way Skia samples a hardware bitmap.
static void sample_external(EGLImageKHR image, PFNGLEGLIMAGETARGETTEXTURE2DOESPROC target) {
    static const float kQuad[] = {-1.0f, -1.0f, 1.0f, -1.0f, -1.0f, 1.0f, 1.0f, 1.0f};
    GLuint p = glCreateProgram();
    glAttachShader(p, shader(GL_VERTEX_SHADER, kQuadVertex));
    glAttachShader(p, shader(GL_FRAGMENT_SHADER, kExternalFragment));
    glBindAttribLocation(p, 0, "pos");
    glLinkProgram(p);
    GLint ok = 0;
    glGetProgramiv(p, GL_LINK_STATUS, &ok);
    CHECK(ok, "external program link");
    glUseProgram(p);
    glUniform1i(glGetUniformLocation(p, "tex"), 0);
    GLint rect = glGetUniformLocation(p, "rect");
    glActiveTexture(GL_TEXTURE0);
    GLuint tex;
    glGenTextures(1, &tex);
    glBindTexture(GL_TEXTURE_EXTERNAL_OES, tex);
    target(GL_TEXTURE_EXTERNAL_OES, (GLeglImageOES)image);
    glViewport(0, 0, W, H);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, kQuad);
    uint8_t pixels[W * H * 4];

    // A fresh texture with no parameters set: an external texture starts
    // with linear filtering, so it is complete (not an opaque black).
    glUniform4f(rect, 0.0f, 0.0f, 1.0f, 1.0f);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    check_frame(pixels, W * 4, 0x0000ffff, "external texture, initial state");

    // A sampler object on the unit replaces the texture's own filtering, as
    // Skia relies on: rows 14..18 of the frame (blue below the triangle's
    // bottom edge at row 16, green above) magnified to the whole height
    // stay pure colors with the sampler's NEAREST, not the texture's LINEAR.
    GLuint sampler;
    glGenSamplers(1, &sampler);
    glSamplerParameteri(sampler, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glSamplerParameteri(sampler, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glBindSampler(0, sampler);
    glUniform4f(rect, 0.5f, 14.0f / H, 0.0f, 4.0f / H);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    for (int y = 0; y < H; y++) {
        uint32_t c = at(pixels, W * 4, W / 2, y);
        CHECK(c == 0x0000ffff || c == 0x00ff00ff, "external texture, sampler: row %d %08x", y, c);
    }
    CHECK(at(pixels, W * 4, W / 2, 0) != at(pixels, W * 4, W / 2, H - 1),
          "external texture, sampler: no edge");
    glBindSampler(0, 0);
    glDeleteSamplers(1, &sampler);
    CHECK(glGetError() == GL_NO_ERROR, "GL error");
    glDeleteTextures(1, &tex);
    glDeleteProgram(p);
    printf("ok AHardwareBuffer sampled as an external texture (initial state, sampler)\n");
}

// Chromium's SurfaceControl path submits AHardwareBuffers with acquire
// fences and reuses them only after their release fences. Exercise that
// driver boundary without SurfaceFlinger: independent producer/consumer
// contexts, a three-buffer pool, and a different raster on every frame.
// This does not test ASurfaceTransaction or compositor layer placement.
// Fence ownership follows EGL_ANDROID_native_fence_sync, revision 3:
// https://registry.khronos.org/EGL/extensions/ANDROID/EGL_ANDROID_native_fence_sync.txt
static int export_fence(EGLDisplay dpy) {
    PFNEGLDUPNATIVEFENCEFDANDROIDPROC duplicate =
        (PFNEGLDUPNATIVEFENCEFDANDROIDPROC)eglGetProcAddress("eglDupNativeFenceFDANDROID");
    CHECK(duplicate != NULL, "eglDupNativeFenceFDANDROID");
    EGLSyncKHR sync = eglCreateSyncKHR(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, NULL);
    CHECK(sync != EGL_NO_SYNC_KHR, "create handoff fence %x", eglGetError());
    glFlush();
    int fd = duplicate(dpy, sync);
    CHECK(fd >= 0, "export handoff fence %x", eglGetError());
    CHECK(eglDestroySyncKHR(dpy, sync), "destroy exported sync %x", eglGetError());
    CHECK(fcntl(fd, F_GETFD) >= 0, "export survives sync destruction");
    return fd;
}

static void import_fence(EGLDisplay dpy, int fd) {
    const EGLint attribs[] = {EGL_SYNC_NATIVE_FENCE_FD_ANDROID, fd, EGL_NONE};
    EGLSyncKHR sync = eglCreateSyncKHR(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, attribs);
    CHECK(sync != EGL_NO_SYNC_KHR, "import handoff fence %x", eglGetError());
    CHECK(eglWaitSyncKHR(dpy, sync, 0), "wait for handoff fence %x", eglGetError());
    CHECK(eglClientWaitSyncKHR(dpy, sync, 0, 0) == EGL_CONDITION_SATISFIED_KHR,
          "handoff fence signaled after server wait");
    CHECK(eglDestroySyncKHR(dpy, sync), "destroy imported sync %x", eglGetError());
    CHECK(fcntl(fd, F_GETFD) == -1 && errno == EBADF, "imported fd closed by EGL");
}

// The host retains the writer until we have observed the pending state.
// Unlike a freshly exported GPU fence, this cannot signal too early.
static int pending_native_fence(void) {
    EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    CHECK(eglInitialize(dpy, NULL, NULL), "pending eglInitialize %x", eglGetError());
    CHECK(eglGetCurrentContext() == EGL_NO_CONTEXT, "no context for imported fence wait");
    const EGLint attribs[] = {EGL_SYNC_NATIVE_FENCE_FD_ANDROID, 3, EGL_NONE};
    EGLSyncKHR sync = eglCreateSyncKHR(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, attribs);
    CHECK(sync != EGL_NO_SYNC_KHR, "import pending fence %x", eglGetError());
    PFNEGLDUPNATIVEFENCEFDANDROIDPROC duplicate =
        (PFNEGLDUPNATIVEFENCEFDANDROIDPROC)eglGetProcAddress("eglDupNativeFenceFDANDROID");
    CHECK(duplicate != NULL, "eglDupNativeFenceFDANDROID");
    int fd = duplicate(dpy, sync);
    CHECK(fd >= 0 && fd != 3, "duplicate pending fence");
    EGLint status = 0;
    CHECK(eglGetSyncAttribKHR(dpy, sync, EGL_SYNC_STATUS_KHR, &status) &&
              status == EGL_UNSIGNALED_KHR, "imported fence starts pending");
    CHECK(eglClientWaitSyncKHR(dpy, sync, 0, 0) == EGL_TIMEOUT_EXPIRED_KHR,
          "pending fence zero-timeout wait");
    CHECK(!fence_signals(fd, 0), "exported copy is also pending");
    uint64_t timestamp = 0;
    CHECK(fence_info(fd, &timestamp) == 0 && timestamp == 0, "active sync_file status and time");
    CHECK(write(4, "P", 1) == 1, "notify host of pending state");
    CHECK(eglClientWaitSyncKHR(dpy, sync, 0, 5000000000ULL) == EGL_CONDITION_SATISFIED_KHR,
          "context-free wait for host signal");
    CHECK(eglGetSyncAttribKHR(dpy, sync, EGL_SYNC_STATUS_KHR, &status) &&
              status == EGL_SIGNALED_KHR, "imported fence becomes signaled");
    CHECK(eglDestroySyncKHR(dpy, sync), "destroy pending import");
    CHECK(fcntl(3, F_GETFD) == -1 && errno == EBADF, "import owns fd 3");
    CHECK(fence_signals(fd, 0), "export outlives imported sync");
    CHECK(fence_info(fd, &timestamp) == 1 && timestamp > 0, "exported fence status and time");
    CHECK(close(fd) == 0 && close(4) == 0, "close pending test descriptors");
    CHECK(eglTerminate(dpy), "terminate pending display");
    printf("ok pending native fence: timeout, context-free wait, ownership\n");
    return 0;
}

static void surface_control_buffers(EGLDisplay dpy, EGLConfig config, EGLContext producer,
                                   EGLSurface pbuffer,
                                   PFNGLEGLIMAGETARGETTEXTURE2DOESPROC target) {
    enum { POOL = 3, FRAMES = 240 };
    AHardwareBuffer* buffers[POOL];
    EGLImageKHR images[POOL];
    GLuint textures[POOL], fbos[POOL], external[POOL];
    int release[POOL] = {-1, -1, -1};
    const AHardwareBuffer_Desc desc = {
        .width = W, .height = H, .layers = 1,
        .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
        .usage = AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE | AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT,
    };
    glGenTextures(POOL, textures);
    glGenFramebuffers(POOL, fbos);
    for (int i = 0; i < POOL; i++) {
        CHECK(AHardwareBuffer_allocate(&desc, &buffers[i]) == 0, "handoff buffer %d", i);
        images[i] = eglCreateImageKHR(dpy, EGL_NO_CONTEXT, EGL_NATIVE_BUFFER_ANDROID,
                                      eglGetNativeClientBufferANDROID(buffers[i]), NULL);
        CHECK(images[i] != EGL_NO_IMAGE_KHR, "handoff image %d: %x", i, eglGetError());
        glBindTexture(GL_TEXTURE_2D, textures[i]);
        target(GL_TEXTURE_2D, (GLeglImageOES)images[i]);
        glBindFramebuffer(GL_FRAMEBUFFER, fbos[i]);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D,
                              textures[i], 0);
        CHECK(glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE,
              "handoff framebuffer %d", i);
    }
    const EGLint context_attribs[] = {EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE};
    EGLContext consumer = eglCreateContext(dpy, config, EGL_NO_CONTEXT, context_attribs);
    CHECK(consumer != EGL_NO_CONTEXT, "consumer context %x", eglGetError());
    const EGLint surface_attribs[] = {EGL_WIDTH, W, EGL_HEIGHT, H, EGL_NONE};
    EGLSurface output = eglCreatePbufferSurface(dpy, config, surface_attribs);
    CHECK(output != EGL_NO_SURFACE, "consumer surface %x", eglGetError());
    CHECK(eglMakeCurrent(dpy, output, output, consumer), "consumer current %x", eglGetError());
    GLuint sample = glCreateProgram();
    glAttachShader(sample, shader(GL_VERTEX_SHADER, kQuadVertex));
    glAttachShader(sample, shader(GL_FRAGMENT_SHADER, kExternalFragment));
    glBindAttribLocation(sample, 0, "pos");
    glLinkProgram(sample);
    GLint linked = 0;
    glGetProgramiv(sample, GL_LINK_STATUS, &linked);
    CHECK(linked, "handoff sampling program");
    glUseProgram(sample);
    glUniform1i(glGetUniformLocation(sample, "tex"), 0);
    glUniform4f(glGetUniformLocation(sample, "rect"), 0, 0, 1, 1);
    glGenTextures(POOL, external);
    for (int i = 0; i < POOL; i++) {
        glBindTexture(GL_TEXTURE_EXTERNAL_OES, external[i]);
        target(GL_TEXTURE_EXTERNAL_OES, (GLeglImageOES)images[i]);
        glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
        glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    }
    static const float quad[] = {-1, -1, 1, -1, -1, 1, 1, 1};
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, quad);
    glViewport(0, 0, W, H);
    glDisable(GL_DITHER);
    uint8_t pixels[W * H * 4];
    for (int frame = 0; frame < FRAMES; frame++) {
        int slot = frame % POOL;
        CHECK(eglMakeCurrent(dpy, pbuffer, pbuffer, producer), "producer current");
        if (release[slot] >= 0) import_fence(dpy, release[slot]);
        glBindFramebuffer(GL_FRAMEBUFFER, fbos[slot]);
        glDisable(GL_DITHER);
        unsigned r = frame, g = 255 - frame, b = frame ^ 0x5a;
        glClearColor(r / 255.0f, g / 255.0f, b / 255.0f, 1);
        glClear(GL_COLOR_BUFFER_BIT);
        glEnable(GL_SCISSOR_TEST);
        glScissor(W / 4, H / 4, W / 2, H / 2);
        glClearColor(0, 1, 0, 1);
        glClear(GL_COLOR_BUFFER_BIT);
        glScissor(0, H - 8, W, 8);
        glClearColor(1, 0, 1, 1);
        glClear(GL_COLOR_BUFFER_BIT);
        glDisable(GL_SCISSOR_TEST);
        int acquire = export_fence(dpy);
        CHECK(glGetError() == GL_NO_ERROR, "producer GL error at frame %d", frame);

        CHECK(eglMakeCurrent(dpy, output, output, consumer), "consumer current");
        import_fence(dpy, acquire);
        glBindTexture(GL_TEXTURE_EXTERNAL_OES, external[slot]);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        release[slot] = export_fence(dpy);
        // No producer readback or glFinish before the handoff. Inspect
        // the consumer's output only after it has submitted its release.
        glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
        CHECK(glGetError() == GL_NO_ERROR, "consumer GL error at frame %d", frame);
        for (int y = 0; y < H; y++) {
            for (int x = 0; x < W; x++) {
                uint32_t want = r << 24 | g << 16 | b << 8 | 0xff;
                if (x >= W / 4 && x < 3 * W / 4 && y >= H / 4 && y < 3 * H / 4)
                    want = 0x00ff00ff;
                if (y >= H - 8) want = 0xff00ffff;
                CHECK(at(pixels, W * 4, x, y) == want,
                      "handoff frame %d slot %d pixel (%d,%d): %08x want %08x",
                      frame, slot, x, y, at(pixels, W * 4, x, y), want);
            }
        }
    }
    glDeleteTextures(POOL, external);
    glDeleteProgram(sample);
    CHECK(eglMakeCurrent(dpy, pbuffer, pbuffer, producer), "restore producer");
    for (int i = 0; i < POOL; i++) import_fence(dpy, release[i]);
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    glDeleteFramebuffers(POOL, fbos);
    glDeleteTextures(POOL, textures);
    for (int i = 0; i < POOL; i++) {
        CHECK(eglDestroyImageKHR(dpy, images[i]), "destroy handoff image");
        AHardwareBuffer_release(buffers[i]);
    }
    CHECK(eglDestroySurface(dpy, output), "destroy consumer surface");
    CHECK(eglDestroyContext(dpy, consumer), "destroy consumer context");
    printf("ok SurfaceControl-style handoff: 240 frames, 3 buffers, acquire/release fences, all pixels\n");
}

int main(int argc, char** argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc > 1 && strcmp(argv[1], "fork") == 0) return zygote();
    if (argc > 1 && strcmp(argv[1], "pending") == 0) return pending_native_fence();
    // 1. A buffer through AHardwareBuffer (libui's Gralloc5: IAllocator
    // over binder, then mapper.aim.so in this process).
    AHardwareBuffer_Desc desc = {
        .width = W, .height = H, .layers = 1,
        .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
        .usage = AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE |
                 AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT |
                 AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN | AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN,
    };
    AHardwareBuffer* ahb = NULL;
    double t0 = now_ns();
    CHECK(AHardwareBuffer_allocate(&desc, &ahb) == 0, "AHardwareBuffer_allocate");
    double alloc_us = (now_ns() - t0) / 1e3;
    AHardwareBuffer_Desc got;
    AHardwareBuffer_describe(ahb, &got);
    uint64_t id = 0;
    AHardwareBuffer_getId(ahb, &id);
    printf("ok allocate %ux%u stride %u id %llx (%.0f us)\n", got.width, got.height, got.stride,
           (unsigned long long)id, alloc_us);
    CHECK(got.stride >= W, "stride");

    // CPU access: write a pattern, read it back through a second lock.
    uint8_t* cpu = NULL;
    CHECK(AHardwareBuffer_lock(ahb, AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN, -1, NULL,
                               (void**)&cpu) == 0, "lock for writing");
    for (int y = 0; y < H; y++) memset(cpu + y * got.stride * 4, y, W * 4);
    CHECK(AHardwareBuffer_unlock(ahb, NULL) == 0, "unlock");
    CHECK(AHardwareBuffer_lock(ahb, AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN, -1, NULL,
                               (void**)&cpu) == 0, "lock for reading");
    CHECK(cpu[37 * got.stride * 4 + 5] == 37, "CPU pattern");
    AHardwareBuffer_unlock(ahb, NULL);
    printf("ok cpu lock/unlock\n");

    // 2. EGL through the original loader, which loads libGLES_aim.so.
    EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    EGLint major = 0, minor = 0;
    CHECK(eglInitialize(dpy, &major, &minor), "eglInitialize %x", eglGetError());
    const EGLint config_attribs[] = {EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8,
                                     EGL_ALPHA_SIZE, 8, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
                                     EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_NONE};
    EGLConfig config;
    EGLint n = 0;
    CHECK(eglChooseConfig(dpy, config_attribs, &config, 1, &n) && n == 1, "eglChooseConfig");
    const EGLint context_attribs[] = {EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE};
    EGLContext ctx = eglCreateContext(dpy, config, EGL_NO_CONTEXT, context_attribs);
    CHECK(ctx != EGL_NO_CONTEXT, "eglCreateContext %x", eglGetError());
    const EGLint pbuffer_attribs[] = {EGL_WIDTH, W, EGL_HEIGHT, H, EGL_NONE};
    EGLSurface pbuffer = eglCreatePbufferSurface(dpy, config, pbuffer_attribs);
    CHECK(pbuffer != EGL_NO_SURFACE, "eglCreatePbufferSurface %x", eglGetError());
    CHECK(eglMakeCurrent(dpy, pbuffer, pbuffer, ctx), "eglMakeCurrent %x", eglGetError());
    printf("ok egl %d.%d %s | %s | %s\n", major, minor, eglQueryString(dpy, EGL_VENDOR),
           glGetString(GL_RENDERER), glGetString(GL_VERSION));

    // 3. Clear and a triangle into the pbuffer; read the pixels back.
    GLuint prog = program();
    uint8_t pixels[W * H * 4];
    frame(prog, 1.0f, 0.0f, 0.0f);
    glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    check_frame(pixels, W * 4, 0xff0000ff, "pbuffer");
    CHECK(glGetError() == GL_NO_ERROR, "GL error");
    printf("ok pbuffer clear + triangle\n");

    // 4. The same into the AHardwareBuffer, as an EGLImage-backed texture.
    EGLClientBuffer client = eglGetNativeClientBufferANDROID(ahb);
    const EGLint image_attribs[] = {EGL_IMAGE_PRESERVED_KHR, EGL_TRUE, EGL_NONE};
    EGLImageKHR image = eglCreateImageKHR(dpy, EGL_NO_CONTEXT, EGL_NATIVE_BUFFER_ANDROID, client,
                                          image_attribs);
    CHECK(image != EGL_NO_IMAGE_KHR, "eglCreateImageKHR %x", eglGetError());
    PFNGLEGLIMAGETARGETTEXTURE2DOESPROC target_texture =
        (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress("glEGLImageTargetTexture2DOES");
    CHECK(target_texture != NULL, "glEGLImageTargetTexture2DOES");
    GLuint tex, fbo;
    glGenTextures(1, &tex);
    glBindTexture(GL_TEXTURE_2D, tex);
    target_texture(GL_TEXTURE_2D, (GLeglImageOES)image);
    glGenFramebuffers(1, &fbo);
    glBindFramebuffer(GL_FRAMEBUFFER, fbo);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);
    CHECK(glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE, "FBO status");
    frame(prog, 0.0f, 0.0f, 1.0f);
    glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    check_frame(pixels, W * 4, 0x0000ffff, "EGLImage readback");
    glFinish();
    // The buffer's memory is the texture: the CPU sees the frame.
    CHECK(AHardwareBuffer_lock(ahb, AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN, -1, NULL,
                               (void**)&cpu) == 0, "lock after rendering");
    check_frame(cpu, got.stride * 4, 0x0000ffff, "AHardwareBuffer memory");
    AHardwareBuffer_unlock(ahb, NULL);
    printf("ok AHardwareBuffer EGLImage clear + triangle (GPU and CPU views)\n");
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    sample_external(image, target_texture);
    surface_control_buffers(dpy, config, ctx, pbuffer, target_texture);

    // 5. Timings: one GL call, and whole triangle frames.
    const int calls = 1000000;
    t0 = now_ns();
    for (int i = 0; i < calls; i++) glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
    printf("timing glClearColor %.1f ns/call\n", (now_ns() - t0) / calls);
    t0 = now_ns();
    for (int i = 0; i < calls; i++) glGetError();
    printf("timing glGetError %.1f ns/call\n", (now_ns() - t0) / calls);
    const int frames = 500;
    t0 = now_ns();
    for (int i = 0; i < frames; i++) {
        frame(prog, 0.0f, 0.0f, 1.0f);
        glFinish();
    }
    printf("timing triangle frame (clear + draw + glFinish) %.1f us\n",
           (now_ns() - t0) / frames / 1e3);
    t0 = now_ns();
    for (int i = 0; i < frames; i++) frame(prog, 0.0f, 0.0f, 1.0f);
    glFinish();
    printf("timing triangle frame, pipelined %.1f us\n", (now_ns() - t0) / frames / 1e3);
    native_fences(dpy, prog);

    // 6. A window surface: an ImageReader's ANativeWindow (a BufferQueue
    // in this process). The frame reaches the queued buffer top row first.
    AImageReader* reader = NULL;
    CHECK(AImageReader_newWithUsage(W, H, AIMAGE_FORMAT_RGBA_8888,
                                    AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT |
                                            AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN,
                                    2, &reader) == AMEDIA_OK, "AImageReader_newWithUsage");
    ANativeWindow* window = NULL;
    CHECK(AImageReader_getWindow(reader, &window) == AMEDIA_OK, "AImageReader_getWindow");
    const EGLint window_config_attribs[] = {EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8,
                                            EGL_ALPHA_SIZE, 8, EGL_RENDERABLE_TYPE,
                                            EGL_OPENGL_ES3_BIT, EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
                                            EGL_NONE};
    EGLConfig window_config;
    CHECK(eglChooseConfig(dpy, window_config_attribs, &window_config, 1, &n) && n == 1,
          "window config");
    EGLSurface surface = eglCreateWindowSurface(dpy, window_config, window, NULL);
    CHECK(surface != EGL_NO_SURFACE, "eglCreateWindowSurface %x", eglGetError());
    CHECK(eglMakeCurrent(dpy, surface, surface, ctx), "eglMakeCurrent window %x", eglGetError());
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    // Red, with a blue band at the top (GL's y = H - 8 .. H) and a triangle.
    frame(prog, 1.0f, 0.0f, 0.0f);
    glEnable(GL_SCISSOR_TEST);
    glScissor(0, H - 8, W, 8);
    glClearColor(0.0f, 0.0f, 1.0f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    glDisable(GL_SCISSOR_TEST);
    t0 = now_ns();
    CHECK(eglSwapBuffers(dpy, surface), "eglSwapBuffers %x", eglGetError());
    double swap_us = (now_ns() - t0) / 1e3;
    AImage* frame_image = NULL;
    CHECK(AImageReader_acquireLatestImage(reader, &frame_image) == AMEDIA_OK, "acquire image");
    AHardwareBuffer* queued = NULL;
    CHECK(AImage_getHardwareBuffer(frame_image, &queued) == AMEDIA_OK, "image buffer");
    AHardwareBuffer_Desc queued_desc;
    AHardwareBuffer_describe(queued, &queued_desc);
    CHECK(AHardwareBuffer_lock(queued, AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN, -1, NULL,
                               (void**)&cpu) == 0, "lock queued buffer");
    uint32_t top = at(cpu, queued_desc.stride * 4, W / 2, 0);
    uint32_t bottom = at(cpu, queued_desc.stride * 4, 1, H - 1);
    uint32_t centre = at(cpu, queued_desc.stride * 4, W / 2, H / 2);
    AHardwareBuffer_unlock(queued, NULL);
    CHECK(top == 0x0000ffff, "window top row %08x", top);
    CHECK(bottom == 0xff0000ff, "window bottom row %08x", bottom);
    CHECK(centre == 0x00ff00ff, "window centre %08x", centre);
    AImage_delete(frame_image);
    printf("ok window surface swap, top row first (swap %.0f us)\n", swap_us);
    const int swaps = 200;
    t0 = now_ns();
    for (int i = 0; i < swaps; i++) {
        frame(prog, 0.0f, 0.0f, 1.0f);
        CHECK(eglSwapBuffers(dpy, surface), "eglSwapBuffers %x", eglGetError());
        if (AImageReader_acquireLatestImage(reader, &frame_image) == AMEDIA_OK)
            AImage_delete(frame_image);
    }
    printf("timing window frame (triangle + eglSwapBuffers + acquire) %.1f us\n",
           (now_ns() - t0) / swaps / 1e3);
    eglMakeCurrent(dpy, pbuffer, pbuffer, ctx);
    eglDestroySurface(dpy, surface);
    AImageReader_delete(reader);

    glDeleteFramebuffers(1, &fbo);
    glDeleteTextures(1, &tex);
    eglDestroyImageKHR(dpy, image);
    eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    AHardwareBuffer_release(ahb);
    printf("ok done\n");
    return 0;
}
