# Android Trace / Perfetto ownership

The runtime uses the pinned AOSP `android_os_Trace.cpp` registrar, including
the CriticalNative calling convention and string handling. Framework tracing
policy and category routing come from `libtracing_perfetto`, not a JNI stub.

The existing pinned Perfetto checkout supplies its public C ABI, built into a
dedicated dylib with POSIX system IPC enabled. Its GN output is isolated from
ANGLE's output. A system tracing service need not be running to launch an app;
without a tracing session, categories remain disabled. The build does not claim
to install/start a system tracing daemon. Linux tracefs is unavailable on macOS;
the existing upstream libcutils host atrace implementation remains inactive.

The one source patch adapts two track-registration calls to the newer public
C API's explicit `is_name_static` argument. Owned strings are marked dynamic.
Original sources remain hash-checked and untouched. C++23 is used because the
Apple standard library requires it to combine C and C++ atomic headers.

`tools/tracing-perfetto-smoke.cc` uses the original in-process test backend and
real tracing sessions. It asserts category enable/disable transitions and reads
back emitted span, instant and counter names after flushing. This is native
contract evidence, not proof of Java/app or system-daemon tracing acceptance.
