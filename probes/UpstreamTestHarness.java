package dev.aim.probe;

import java.io.OutputStream;
import java.io.PrintStream;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.Set;

/** Process-local output capture for unmodified pinned AOSP ART run tests. */
public final class UpstreamTestHarness {
    private UpstreamTestHarness() {}

    private static PrintStream originalOut;
    private static PrintStream originalErr;
    private static Thread shutdownHook;
    private static boolean outputPrepared;
    private static boolean dispatchingOnJavaThread;

    public static Class<?> load(String className) throws ClassNotFoundException {
        return Class.forName(className, false, Thread.currentThread().getContextClassLoader());
    }

    private static native void writeOutputByte(int channel, int value);
    private static native void writeOutputChunk(int channel, byte[] bytes, int offset, int length);

    private static void flushOutput() {
        System.out.flush();
        System.err.flush();
    }

    private static final class NativeOutputStream extends OutputStream {
        private final int channel;
        private final StringBuilder pending = new StringBuilder();
        private String pendingMethodInvoke;
        private int suppressedFrames;
        private boolean sawCause;

        NativeOutputStream(int channel) { this.channel = channel; }

        @Override public void write(int value) {
            byte[] one = {(byte) value};
            write(one, 0, 1);
        }

        @Override public void write(byte[] bytes, int offset, int length) {
            if (bytes == null) throw new NullPointerException("bytes");
            if ((offset | length) < 0 || length > bytes.length - offset) {
                throw new IndexOutOfBoundsException();
            }
            if (length == 0) return;
            pending.append(new String(bytes, offset, length, StandardCharsets.UTF_8));
            int newline;
            while ((newline = pending.indexOf("\n")) >= 0) {
                emitLine(pending.substring(0, newline + 1));
                pending.delete(0, newline + 1);
            }
        }

        @Override public void flush() {
            if (pending.length() != 0) {
                emitLine(pending.toString());
                pending.setLength(0);
            }
            if (pendingMethodInvoke != null) {
                emitVisibleLine(pendingMethodInvoke);
                pendingMethodInvoke = null;
            }
        }

        private void emitLine(String line) {
            // Method.invoke is also a legitimate application frame (for
            // example, tests that exercise reflection). Delay that line until
            // the following frame identifies the harness dispatch itself.
            // The old unconditional filter accidentally removed app frames.
            if (pendingMethodInvoke != null) {
                if (line.contains("\tat " + UpstreamTestHarness.class.getName()
                        + "$TestMainThread.run")
                        || line.contains("\tat " + UpstreamTestHarness.class.getName()
                        + ".run")) {
                    suppressedFrames++;
                    pendingMethodInvoke = null;
                } else {
                    emitVisibleLine(pendingMethodInvoke);
                    pendingMethodInvoke = null;
                }
            }
            if (line.contains("\tat java.lang.reflect.Method.invoke")) {
                pendingMethodInvoke = line;
                return;
            }
            // The Java-thread dispatch is a host implementation detail. ART's
            // direct dalvikvm launcher does not expose these reflection frames;
            // suppress them at the output boundary and preserve Throwable's
            // common-frame count for AOSP-compatible diagnostics.
            if (line.contains("\tat java.lang.reflect.Method.invoke")
                    || line.contains("\tat " + UpstreamTestHarness.class.getName()
                    + "$TestMainThread.run")) {
                suppressedFrames++;
                return;
            }
            if (line.startsWith("Caused by: ")) sawCause = true;
            if (line.startsWith("\t... ") && line.endsWith(" more\n")) {
                int start = 5;
                int end = line.indexOf(" more", start);
                try {
                    int common = Integer.parseInt(line.substring(start, end));
                    // A cause can elide the dispatch frames entirely, so they
                    // are not visible to the line filter even though they are
                    // included in Throwable's precomputed common count.
                    if (suppressedFrames == 0 && dispatchingOnJavaThread && sawCause && common >= 2) {
                        common -= 2;
                    } else {
                        common = Math.max(0, common - suppressedFrames);
                    }
                    line = "\t... " + common + " more\n";
                } catch (NumberFormatException ignored) {
                    // Preserve malformed application output verbatim.
                }
                suppressedFrames = 0;
            }
            emitVisibleLine(line);
        }

        private void emitVisibleLine(String line) {
            byte[] encoded = line.getBytes(StandardCharsets.UTF_8);
            writeOutputChunk(channel, encoded, 0, encoded.length);
        }
    }

    private static final class OutputShutdownHook extends Thread {
        @Override public void run() { flushOutput(); }
    }

    public static void prepareOutput() {
        if (outputPrepared) return;
        originalOut = System.out;
        originalErr = System.err;
        shutdownHook = new OutputShutdownHook();
        System.setOut(new PrintStream(new NativeOutputStream(0), true));
        System.setErr(new PrintStream(new NativeOutputStream(1), true));
        Runtime.getRuntime().addShutdownHook(shutdownHook);
        outputPrepared = true;
    }

    private static final class TestMainThread extends Thread {
        private final String className;
        private final String[] arguments;
        private volatile Throwable failure;
        private volatile boolean failureOutputDispatched;

        TestMainThread(String className, String[] arguments) {
            // dalvikvm invokes the application entry point on the process
            // main thread. Keep that observable identity (and pthread name)
            // instead of exposing the harness implementation name.
            super("main");
            this.className = className;
            this.arguments = arguments;
        }

        @Override public void run() {
            try {
                Class<?> mainClass = load(className);
                Method main = mainClass.getDeclaredMethod("main", String[].class);
                main.setAccessible(true);
                main.invoke(null, (Object) arguments);
            } catch (InvocationTargetException exception) {
                failure = exception.getCause();
                stripHarnessFrames(failure);
                failureOutputDispatched = dispatchExplicitUncaughtException(failure);
            } catch (Throwable throwable) {
                failure = throwable;
                stripHarnessFrames(failure);
                failureOutputDispatched = dispatchExplicitUncaughtException(failure);
            }
        }
    }

    // Reflection is only the host-side dispatch mechanism. Android's
    // dalvikvm invokes the application entry point directly, so its internal
    // Method.invoke/TestMainThread frames must not leak into observable
    // Throwable output or alter the common-frame elision count.
    private static void stripHarnessFrames(Throwable throwable) {
        for (Throwable current = throwable; current != null; current = current.getCause()) {
            StackTraceElement[] source = current.getStackTrace();
            int kept = 0;
            for (StackTraceElement element : source) {
                String className = element.getClassName();
                if (className.equals("java.lang.reflect.Method")
                        || className.equals(UpstreamTestHarness.class.getName() + "$TestMainThread")) {
                    continue;
                }
                source[kept++] = element;
            }
            if (kept != source.length) {
                StackTraceElement[] trimmed = new StackTraceElement[kept];
                System.arraycopy(source, 0, trimmed, 0, kept);
                current.setStackTrace(trimmed);
            }
        }
    }

    private static void awaitNewNonDaemonThreads(Set<Thread> baseline) throws InterruptedException {
        while (true) {
            Set<Thread> running = Thread.getAllStackTraces().keySet();
            Thread pending = null;
            for (Thread thread : running) {
                if (!baseline.contains(thread) && thread != Thread.currentThread()
                        && thread.isAlive() && !thread.isDaemon()) {
                    pending = thread;
                    break;
                }
            }
            if (pending == null) return;
            pending.join();
        }
    }

    public static Set<Thread> beginRun() {
        prepareOutput();
        return new HashSet<>(Thread.getAllStackTraces().keySet());
    }

    public static boolean dispatchExplicitUncaughtException(Throwable failure) {
        Thread thread = Thread.currentThread();
        Thread.UncaughtExceptionHandler handler = thread.getUncaughtExceptionHandler();
        // ThreadGroup is Java's implicit fallback. A dalvikvm-style launcher
        // reports an unhandled Main exception as failure. ThreadGroup must
        // still run when an application installed the process-wide default
        // handler because its uncaughtException() delegates to that handler.
        if (handler == null || (handler == thread.getThreadGroup()
                && Thread.getDefaultUncaughtExceptionHandler() == null)) return false;
        handler.uncaughtException(thread, failure);
        return true;
    }

    public static Throwable finishRun(Set<Thread> baseline, Throwable failure) {
        // AOSP keeps a process alive while application-created non-daemon
        // threads are running. The Darwin host cannot call DestroyJavaVM for
        // an Android process (the process lifetime is the shutdown boundary),
        // so perform the same join before publishing captured output and
        // returning to the host's _exit path. This is observable Android
        // process semantics, not a test-specific delay.
        try {
            awaitNewNonDaemonThreads(baseline);
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            if (failure == null) failure = interrupted;
        }
        if (failure != null) {
            // Match dalvikvm's owner-thread uncaught-exception presentation.
            // The native launcher uses the returned Throwable only to select
            // its exit status and must not print it a second time.
            System.err.print("Exception in thread \"main\" ");
            failure.printStackTrace(System.err);
        }
        flushOutput();
        // Keep the descriptor bridge installed through owner-thread detach and
        // VM destruction. JVMTI tests intentionally observe that final phase.
        return failure;
    }

    public static Throwable run(String className) {
        return run(className, new String[0]);
    }

    public static Throwable run(String className, String[] arguments) {
        Throwable failure = null;
        boolean failureOutputDispatched = false;
        try {
            prepareOutput();
            dispatchingOnJavaThread = true;
            Set<Thread> baseline = new HashSet<>(Thread.getAllStackTraces().keySet());
            // dalvikvm executes the application entry point directly on its
            // attached process-main peer. Besides matching Android's thread
            // identity, this keeps method tracing from recording the harness
            // worker/join frames as part of the application's main trace.
            Class<?> mainClass = load(className);
            Method main = mainClass.getDeclaredMethod("main", String[].class);
            main.setAccessible(true);
            try {
                main.invoke(null, (Object) arguments);
            } catch (InvocationTargetException exception) {
                failure = exception.getCause();
                stripHarnessFrames(failure);
                failureOutputDispatched = dispatchExplicitUncaughtException(failure);
            }
            awaitNewNonDaemonThreads(baseline);
        } catch (Throwable throwable) {
            failure = throwable;
        } finally {
            dispatchingOnJavaThread = false;
            if (failure != null && !failureOutputDispatched) {
                System.err.print("Exception in thread \"main\" ");
                failure.printStackTrace(System.err);
            }
            flushOutput();
            if (outputPrepared) {
                System.setOut(originalOut);
                System.setErr(originalErr);
                Runtime.getRuntime().removeShutdownHook(shutdownHook);
                outputPrepared = false;
            }
        }
        return failure;
    }
}
