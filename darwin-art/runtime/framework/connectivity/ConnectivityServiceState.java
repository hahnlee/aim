package dev.darwinart.runtime.connectivity;

import android.os.Handler;
import android.os.HandlerThread;
import android.os.SystemClock;
import java.util.ArrayList;

/** Android-owned connectivity state composed from host facts and validation observations. */
public final class ConnectivityServiceState implements ConnectivityState, AutoCloseable {
    private static final long PATH_POLL_MILLIS = 1000;
    private static final long VALIDATION_RETRY_MILLIS = 10000;

    private final ConnectivityState hostFacts;
    private final HandlerThread ownerThread;
    private final Handler owner;
    private final NetworkValidationMonitor validation;
    private final ArrayList<Listener> listeners = new ArrayList<>();
    private volatile ConnectivitySnapshot published = ConnectivitySnapshot.unavailable();
    private boolean closed;
    private long lastValidationStart = Long.MIN_VALUE;

    public ConnectivityServiceState(ConnectivityState hostState) {
        this(hostState, new HttpNetworkProbeTransport());
    }

    ConnectivityServiceState(ConnectivityState hostState, NetworkProbeTransport transport) {
        if (hostState == null) throw new NullPointerException("hostState");
        if (transport == null) throw new NullPointerException("transport");
        hostFacts = hostState;
        ownerThread = new HandlerThread("ConnectivityServiceState");
        ownerThread.start();
        owner = new Handler(ownerThread.getLooper());
        validation = new NetworkValidationMonitor(owner, transport,
                new NetworkValidationMonitor.Callback() {
                    @Override
                    public void onValidationResult(long generation,
                            NetworkProbeTransport.Result result) {
                        applyValidationResult(generation, result);
                    }
                });
        owner.post(new Runnable() {
            @Override
            public void run() { refreshAndSchedule(); }
        });
    }

    @Override
    public boolean isActiveNetworkMetered() { return published.isMetered(); }

    @Override
    public ConnectivitySnapshot snapshot() { return published; }

    @Override
    public synchronized void addListener(Listener listener) {
        if (listener == null) throw new NullPointerException("listener");
        if (!closed && !listeners.contains(listener)) listeners.add(listener);
    }

    @Override
    public synchronized void removeListener(Listener listener) {
        listeners.remove(listener);
    }

    private void refreshAndSchedule() {
        if (closed) return;
        ConnectivitySnapshot observed = hostFacts.snapshot();
        ConnectivitySnapshot current = published;
        if (!samePathGeneration(current, observed)) {
            publish(observed.withValidation(false));
            current = published;
            lastValidationStart = Long.MIN_VALUE;
        }
        long now = SystemClock.elapsedRealtime();
        if (current.hasActiveNetwork() && !current.isValidated() && !validation.isInFlight()
                && retryDue(now)) {
            lastValidationStart = now;
            validation.evaluate(current.generation());
        }
        owner.postDelayed(new Runnable() {
            @Override
            public void run() { refreshAndSchedule(); }
        }, PATH_POLL_MILLIS);
    }

    private boolean retryDue(long now) {
        return lastValidationStart == Long.MIN_VALUE
                || now - lastValidationStart >= VALIDATION_RETRY_MILLIS;
    }

    private void applyValidationResult(long generation, NetworkProbeTransport.Result result) {
        if (closed) return;
        ConnectivitySnapshot current = published;
        if (!current.hasActiveNetwork() || current.generation() != generation) return;
        boolean validated = result == NetworkProbeTransport.Result.VALIDATED;
        if (validated != current.isValidated()) publish(current.withValidation(validated));
    }

    private void publish(ConnectivitySnapshot snapshot) {
        published = snapshot;
        ArrayList<Listener> copy;
        synchronized (this) {
            if (closed) return;
            copy = new ArrayList<>(listeners);
        }
        for (Listener listener : copy) listener.onConnectivityChanged(snapshot);
    }

    private static boolean samePathGeneration(ConnectivitySnapshot first,
            ConnectivitySnapshot second) {
        return first.hasActiveNetwork() == second.hasActiveNetwork()
                && first.generation() == second.generation();
    }

    @Override
    public void close() {
        synchronized (this) {
            if (closed) return;
            closed = true;
            listeners.clear();
        }
        owner.removeCallbacksAndMessages(null);
        validation.close();
        ownerThread.quitSafely();
        if (hostFacts instanceof AutoCloseable) {
            try {
                ((AutoCloseable) hostFacts).close();
            } catch (Exception ignored) {
                // Process shutdown is already retiring the system service owner.
            }
        }
    }
}
