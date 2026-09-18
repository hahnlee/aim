package dev.darwinart.runtime.wm;

import android.os.Handler;
import android.os.HandlerThread;
import java.util.ArrayList;

/** WMS transport progress/lifetime, independent of Binder callbacks and window policy. */
final class WindowPublicationDriver implements WindowFocusPublicationDelivery.Transport {
    private final WindowFocusRegistry registry;
    private final WindowFocusPublicationDelivery delivery;
    private final Handler handler;
    private final ArrayList<WindowInputEndpoint> endpoints = new ArrayList<>();
    private boolean scheduled;
    private long workGeneration;
    private int retryDelay = 4;
    private Throwable failure;
    private final Runnable turn = this::runTurn;

    WindowPublicationDriver(WindowFocusRegistry registry) {
        this.registry = registry;
        delivery = new WindowFocusPublicationDelivery(registry, this);
        HandlerThread thread = new HandlerThread("WindowManagerInputPublication");
        thread.start();
        handler = new Handler(thread.getLooper());
    }

    synchronized void retain(WindowInputEndpoint endpoint) {
        endpoints.add(endpoint);
    }

    synchronized boolean owns(android.view.InputChannel channel) {
        for (WindowInputEndpoint endpoint : endpoints) if (endpoint.owns(channel)) return true;
        return false;
    }

    synchronized void retire(WindowInputEndpoint endpoint) {
        endpoint.retired = true;
    }

    synchronized void checkHealthy() {
        if (failure != null) throw new IllegalStateException("WMS publication driver failed", failure);
    }

    synchronized void wake() {
        checkHealthy();
        ++workGeneration;
        retryDelay = 4;
        schedule(0);
    }

    synchronized void fail(Throwable error) {
        if (failure == null) failure = error;
    }

    /** Executes policy on the actual WMS handler without owning policy itself. */
    synchronized void executePolicy(Runnable action) {
        checkHealthy();
        try {
            if (handler.postDelayed(() -> {
                try {
                    synchronized (WindowPublicationDriver.this) { checkHealthy(); }
                    action.run();
                }
                catch (Throwable error) {
                    synchronized (WindowPublicationDriver.this) {
                        if (failure == null) failure = error;
                    }
                    android.util.Log.e("WindowManager", "Root activation policy failed", error);
                }
            }, 0)) return;
            throw new IllegalStateException("WMS Looper rejected root activation");
        } catch (Throwable rejected) {
            if (failure == null) failure = rejected;
            throw rejected;
        }
    }

    private void schedule(int delay) {
        if (scheduled) return;
        scheduled = true;
        try {
            if (handler.postDelayed(turn, delay)) return;
            throw new IllegalStateException("WMS publication Looper rejected progress");
        } catch (Throwable rejected) {
            scheduled = false;
            if (failure == null) failure = rejected;
            throw rejected;
        }
    }

    @Override public WindowFocusPublicationDelivery.Result publish(
            WindowFocusRegistry.Publication record) {
        return endpoint(record).publish(record);
    }

    @Override public boolean terminateAndQuiesce(WindowFocusRegistry.Publication record) {
        return endpoint(record).terminateAndQuiesce();
    }

    private static WindowInputEndpoint endpoint(WindowFocusRegistry.Publication record) {
        if (!(record.channelIncarnation instanceof WindowInputEndpoint))
            throw new IllegalArgumentException("publication has no original WMS lease");
        return (WindowInputEndpoint) record.channelIncarnation;
    }

    private void runTurn() {
        try {
            final ArrayList<WindowInputEndpoint> snapshot;
            final long generation;
            synchronized (this) {
                checkHealthy();
                // Remain scheduled during IO; generation guards coalesced ADD.
                snapshot = new ArrayList<>(endpoints);
                generation = workGeneration;
            }
            for (WindowInputEndpoint endpoint : snapshot) endpoint.flush();
            WindowFocusPublicationDelivery.Attempt terminal = delivery.pendingTerminalAttempt();
            if (terminal != null) delivery.settleTerminal(terminal);
            delivery.drain(32);
            boolean pendingTx = false;
            for (WindowInputEndpoint endpoint : snapshot) {
                int state = endpoint.acceptedPrefix();
                if (state < 0 || state > 2)
                    throw new IllegalStateException("invalid original accepted-prefix query: " + state);
                boolean retired;
                synchronized (this) { retired = endpoint.retired; }
                if (retired && !registry.referencesChannel(endpoint)) {
                    boolean settled = state == 0
                            || (state == 2 && endpoint.terminateAndQuiesce());
                    if (settled) {
                        endpoint.release();
                        synchronized (this) { endpoints.remove(endpoint); }
                    } else pendingTx = true;
                } else if (state == 1) pendingTx = true;
            }
            synchronized (this) {
                scheduled = false;
                // Includes the last REMOVE: no future Binder call is required.
                boolean retiring = false;
                for (WindowInputEndpoint endpoint : endpoints) retiring |= endpoint.retired;
                if (generation != workGeneration || pendingTx || retiring
                        || registry.pendingBatch() != null) {
                    schedule(retryDelay);
                    retryDelay = Math.min(64, retryDelay * 2);
                }
            }
        } catch (Throwable error) {
            synchronized (this) {
                if (failure == null) failure = error;
                scheduled = false;
            }
            // Preserve all leases/queue obligations and expose the invariant
            // failure, rather than acknowledging or disposing uncertain work.
            android.util.Log.e("WindowManager", "Input publication driver failed", error);
        }
    }
}
