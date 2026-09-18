package dev.darwinart.runtime.wm;

import android.os.IBinder;
import android.os.RemoteException;
import java.util.IdentityHashMap;

/** WMS-owned retained root decisions; every state access holds the controller monitor. */
final class WindowRootFocusDecisions {
    interface Channels { IBinder originalToken(WindowFocusRegistry.WindowSpec selected); }

    private static final class Subscription {
        final DesktopRootRegistry.Registration root;
        final IBinder callback;
        long sequence;
        DesktopRootFocusDecision latest;
        boolean pending;
        boolean scheduled;
        int retryDelay = 4;
        Subscription(DesktopRootRegistry.Registration root, IBinder callback) {
            this.root = root;
            this.callback = callback;
        }
    }

    private final Object monitor;
    private final WindowFocusRegistry registry;
    private final WindowRootActivationPolicy activation;
    private final WindowPublicationDriver driver;
    private final Channels channels;
    private final WindowRootFocusDecisionDelivery delivery = new WindowRootFocusDecisionDelivery();
    private final IdentityHashMap<DesktopRootRegistry.Registration, Subscription> subscriptions =
            new IdentityHashMap<>();

    WindowRootFocusDecisions(Object monitor, WindowFocusRegistry registry,
            WindowRootActivationPolicy activation, WindowPublicationDriver driver, Channels channels) {
        this.monitor = monitor;
        this.registry = registry;
        this.activation = activation;
        this.driver = driver;
        this.channels = channels;
    }

    void attach(DesktopRootRegistry.Registration root, IBinder callback) {
        driver.checkHealthy();
        if (!root.bindingOpen()) throw new DesktopRootRegistry.BindingRejected("root is terminal");
        Subscription existing = subscriptions.get(root);
        if (existing != null) {
            if (!existing.callback.equals(callback))
                throw new DesktopRootRegistry.BindingRejected("root callback replacement rejected");
            existing.pending = true; // Replay after an uncertain handshake reply.
            captureAll();
            return;
        }
        Subscription added = new Subscription(root, callback);
        subscriptions.put(root, added);
        captureAll();
    }

    void captureAll() {
        try {
            Subscription[] all = subscriptions.values().toArray(new Subscription[subscriptions.size()]);
            for (Subscription subscription : all) {
                if (!subscription.root.bindingOpen()) subscriptions.remove(subscription.root);
                else capture(subscription);
            }
        } catch (RuntimeException | Error error) {
            // A focus change may already have committed. Never silently keep
            // sending a predecessor grant after snapshot/scheduling failure.
            driver.fail(error);
            throw error;
        }
    }

    /** Authenticated terminal cleanup retires only this exact root subscription. */
    void retire(DesktopRootRegistry.Registration root) {
        subscriptions.remove(root);
    }

    private void capture(Subscription subscription) {
        DesktopRootRegistry.Registration root = subscription.root;
        WindowFocusRegistry.DecisionSnapshot selected = registry.decisionSnapshot(0);
        DesktopRootRegistry.Fact retained = root.latestFact;
        DesktopRootRegistry.Fact active = activation.decisionFact(root);
        IBinder token = null;
        long serial = retained == null ? 0 : retained.serial;
        if (selected.window != null && selected.window.rootToken == root && active != null) {
            token = channels.originalToken(selected.window);
            if (token == null) throw new IllegalStateException("selected root has no original channel");
            serial = active.serial;
        }
        DesktopRootFocusDecision old = subscription.latest;
        if (old == null || old.factSerial != serial || old.epoch != selected.epoch
                || !sameToken(old.originalChannelToken, token)) {
            if (subscription.sequence == Long.MAX_VALUE)
                throw new IllegalStateException("root decision sequence exhausted");
            DesktopRootFocusDecision next = new DesktopRootFocusDecision(root.incarnation,
                    serial, subscription.sequence + 1, token, selected.epoch);
            subscription.sequence = next.sequence;
            subscription.latest = next;
            subscription.pending = true;
            subscription.retryDelay = 4;
        }
        if (subscription.pending) schedule(subscription, 0);
    }

    private static boolean sameToken(IBinder a, IBinder b) {
        return a == b || a != null && a.equals(b);
    }

    private void schedule(Subscription subscription, int delay) {
        if (subscription.scheduled) return;
        subscription.scheduled = true;
        try { delivery.schedule(() -> dispatch(subscription), delay); }
        catch (RuntimeException | Error error) {
            subscription.scheduled = false;
            driver.fail(error);
            throw error;
        }
    }

    private void dispatch(Subscription subscription) {
        try {
            final DesktopRootFocusDecision sent;
            synchronized (monitor) {
                driver.checkHealthy();
                if (subscriptions.get(subscription.root) != subscription) return;
                if (!subscription.root.bindingOpen()) {
                    subscriptions.remove(subscription.root);
                    return;
                }
                sent = subscription.latest;
            }
            boolean accepted = false;
            try { accepted = DesktopRootFocusDecisionTransport.send(subscription.callback, sent); }
            catch (RemoteException uncertain) {
                android.util.Log.w("WindowManager", "Root decision delivery uncertain", uncertain);
            }
            catch (RuntimeException rejected) {
                // Exceptions returned by an app callback are not a WMS
                // invariant failure. Retain/retry only this subscription.
                android.util.Log.w("WindowManager", "Root decision callback rejected delivery", rejected);
            }
            synchronized (monitor) {
                if (subscriptions.get(subscription.root) != subscription) return;
                subscription.scheduled = false;
                if (!subscription.root.bindingOpen()) {
                    subscriptions.remove(subscription.root);
                    return;
                }
                if (accepted && subscription.latest == sent) subscription.pending = false;
                if (subscription.pending) {
                    int delay = accepted ? 0 : subscription.retryDelay;
                    subscription.retryDelay = Math.min(128, subscription.retryDelay * 2);
                    schedule(subscription, delay);
                }
            }
        } catch (RuntimeException | Error error) {
            driver.fail(error);
            android.util.Log.e("WindowManager", "Root decision publication failed", error);
        }
    }
}
