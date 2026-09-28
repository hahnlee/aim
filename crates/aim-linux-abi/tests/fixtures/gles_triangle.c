// An NDK program for tests/graphics.rs: graphics buffers through the
// original libnativewindow/libui (allocator and mapper HALs), and GLES
// through the original libEGL loader and the vendor driver.
//
//   gles_triangle          allocate, render, check pixels, print timings
//   gles_triangle fork     zygote's pattern: get the display, fork, and
//                          compile and draw in the child (twice: a display
//                          only got, and one initialized, before the fork)
//
// Prints one "ok ..." line per check and "timing ..." lines; exits non-zero
// on the first failure.

#define EGL_EGLEXT_PROTOTYPES
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <GLES2/gl2ext.h>
#include <android/hardware_buffer.h>
#include <media/NdkImageReader.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
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
    CHECK(ok, "shader compile");
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

// In a fork child: a context on `dpy` (initialized here unless `inited`),
// a program whose fragment shader no cache has seen (a constant unique to
// this run), and a triangle read back. Exits with the result.
static void child_draws(EGLDisplay dpy, int inited) {
    EGLint major = 0, minor = 0;
    if (!inited) CHECK(eglInitialize(dpy, &major, &minor), "child eglInitialize %x", eglGetError());
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
    char fs[256];
    snprintf(fs, sizeof(fs),
             "#version 300 es\nprecision highp float;\nout vec4 color;\nuniform float u%lld;\n"
             "void main() { color = vec4(0.0, 1.0, 0.0, 1.0) + vec4(u%lld * %d.0); }\n",
             (long long)now_ns(), (long long)now_ns(), getpid());
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

int main(int argc, char** argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc > 1 && strcmp(argv[1], "fork") == 0) return zygote();
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
