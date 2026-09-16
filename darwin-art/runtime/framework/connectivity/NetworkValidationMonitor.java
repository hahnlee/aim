package dev.darwinart.runtime.connectivity;

import android.os.Handler;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ThreadFactory;

/** Runs bounded network probes off the connectivity owner thread. */
final class NetworkValidationMonitor implements AutoCloseable {
    interface Callback {
        void onValidationResult(long generation, NetworkProbeTransport.Result result);
    }

    private final Handler owner;
    private final NetworkProbeTransport transport;
    private final Callback callback;
    private final ExecutorService worker;
    private boolean closed;
    private boolean inFlight;

    NetworkValidationMonitor(Handler ownerHandler, NetworkProbeTransport probeTransport,
            Callback resultCallback) {
        if (ownerHandler == null) throw new NullPointerException("ownerHandler");
        if (probeTransport == null) throw new NullPointerException("probeTransport");
        if (resultCallback == null) throw new NullPointerException("resultCallback");
        owner = ownerHandler;
        transport = probeTransport;
        callback = resultCallback;
        worker = Executors.newSingleThreadExecutor(new ThreadFactory() {
            @Override
            public Thread newThread(Runnable work) {
                Thread thread = new Thread(work, "NetworkValidationProbe");
                thread.setDaemon(true);
                return thread;
            }
        });
    }

    boolean evaluate(final long generation) {
        if (closed || inFlight) return false;
        inFlight = true;
        worker.execute(new Runnable() {
            @Override
            public void run() {
                final NetworkProbeTransport.Result result = transport.probe();
                owner.post(new Runnable() {
                    @Override
                    public void run() {
                        if (closed) return;
                        inFlight = false;
                        callback.onValidationResult(generation, result);
                    }
                });
            }
        });
        return true;
    }

    boolean isInFlight() { return inFlight; }

    @Override
    public void close() {
        if (closed) return;
        closed = true;
        inFlight = false;
        worker.shutdownNow();
    }
}
