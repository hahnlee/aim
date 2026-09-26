package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.view.InputChannel;
import android.view.WindowManager;
import java.util.HashMap;
import java.util.IdentityHashMap;

/** Service-wide WMS window snapshots; host-root authentication is a separate join. */
final class WindowPublicationController implements DesktopRootWindowBindingOwner {
    private static final class Entry {
        final WindowSessionWindowOwnership.Registration registration;
        final WindowSessionIdentity identity;
        final WindowInputEndpoint endpoint;
        final android.os.IBinder channelToken;
        final int displayId;
        final Object parent;
        final int type;
        boolean live;
        WindowFocusRegistry.WindowSpec latestSpec;
        DesktopRootRegistry.Registration boundRoot;
        int boundDisplayId;

        Entry(WindowSessionWindowOwnership.Registration registration, WindowSessionIdentity identity,
                WindowInputEndpoint endpoint, android.os.IBinder channelToken, int displayId,
                Object parent, int type) {
            this.registration = registration;
            this.identity = identity;
            this.endpoint = endpoint;
            this.channelToken = channelToken;
            this.displayId = displayId;
            this.parent = parent;
            this.type = type;
        }
    }

    private final WindowFocusRegistry registry = new WindowFocusRegistry();
    private final WindowPublicationDriver driver = new WindowPublicationDriver(registry);
    private final WindowIdRegistry windowIds = new WindowIdRegistry(registry);
    {
        driver.setFocusListener(windowIds);
    }
    private final WindowRootActivationPolicy activationPolicy = new WindowRootActivationPolicy(registry);
    private final WindowRootFocusDecisions rootDecisions = new WindowRootFocusDecisions(
            this, registry, activationPolicy, driver, this::originalToken);
    private final HashMap<Object, Entry> entries = new HashMap<>();
    private final HashMap<android.os.IBinder, Entry> channelEntries = new HashMap<>();
    private final IdentityHashMap<DesktopRootRegistry.Registration,
            IdentityHashMap<Entry, Boolean>> rootBindings = new IdentityHashMap<>();

    DesktopRootWindowBindingOwner bindingOwner() { return this; }

    boolean ownsOriginal(InputChannel original) { return driver.owns(original); }

    synchronized void register(WindowSessionWindowOwnership.Registration registration,
            WindowSessionIdentity identity, InputChannel original,
            WindowManager.LayoutParams attrs, int displayId) {
        driver.checkHealthy();
        if (displayId != 0) throw new IllegalArgumentException("unknown logical display");
        if (entries.containsKey(registration.window()))
            throw new IllegalStateException("window publication already registered");
        WindowManager.LayoutParams effective = attrs == null ? new WindowManager.LayoutParams() : attrs;
        if (!isAttached(effective.type) && (effective.type < 1 || effective.type > 99))
            throw new IllegalArgumentException("window type requires an unsupported WMS policy owner");
        Object parent = null;
        if (isAttached(effective.type)) {
            Entry checked = entries.get(effective.token);
            if (checked == null || !checked.live || checked.parent != null
                    || !identity.sameAttachment(checked.identity))
                throw new SecurityException("attached window has no canonical same-attachment parent");
            parent = checked.registration;
        }
        android.os.IBinder channelToken = WindowInputEndpoint.tokenOf(original);
        if (channelEntries.containsKey(channelToken))
            throw new IllegalStateException("input channel token is already owned");
        WindowInputEndpoint endpoint = new WindowInputEndpoint(original);
        try {
            driver.retain(endpoint);
        } catch (RuntimeException | Error error) {
            // No publication exists yet; an unused lease has no TX obligation.
            endpoint.release();
            throw error;
        }
        try {
            Entry entry = new Entry(registration, identity, endpoint, channelToken, displayId,
                    parent, effective.type);
            entry.latestSpec = spec(entry, new Rect(), android.view.View.GONE, effective.flags);
            entries.put(registration.window(), entry);
            channelEntries.put(channelToken, entry);
            invalidateStaleActivation();
            registry.upsert(entry.latestSpec);
            rootDecisions.captureAll();
        } catch (RuntimeException | Error error) {
            entries.remove(registration.window());
            channelEntries.remove(channelToken);
            driver.retire(endpoint);
            try { driver.wake(); } catch (RuntimeException cleanup) { error.addSuppressed(cleanup); }
            throw error;
        }
        driver.wake();
    }

    synchronized void relayout(WindowSessionWindowOwnership.Registration registration,
            Rect frame, int visibility, WindowManager.LayoutParams attrs) {
        validateRelayout(registration, attrs);
        Entry entry = require(registration);
        WindowFocusRegistry.WindowSpec next = spec(entry, frame, visibility,
                attrs == null ? 0 : attrs.flags);
        if (entry.boundRoot != null) next = paired(next, entry.boundRoot);
        invalidateStaleActivation();
        registry.upsert(next);
        entry.latestSpec = next;
        rootDecisions.captureAll();
        driver.wake();
        reconcileBoundRoot(entry);
    }

    synchronized void ready(WindowSessionWindowOwnership.Registration registration) {
        require(registration).live = true;
    }

    synchronized void validateRelayout(WindowSessionWindowOwnership.Registration registration,
            WindowManager.LayoutParams attrs) {
        driver.checkHealthy();
        Entry entry = require(registration);
        if (attrs != null && attrs.type != entry.type)
            throw new IllegalArgumentException("relayout cannot change window type");
        if (entry.parent != null && attrs != null) {
            WindowSessionWindowOwnership.Registration parent =
                    (WindowSessionWindowOwnership.Registration) entry.parent;
            // Retain the authenticated original parent, never resolve a
            // recycled Binder to a successor during a later relayout.
            if (!parent.window().equals(attrs.token))
                throw new SecurityException("relayout has no original canonical parent");
        }
    }

    /** IWindowSession.getWindowId: the window's IWindowId. */
    android.view.IWindowId windowId(WindowSessionWindowOwnership.Registration registration) {
        return windowIds.windowId(registration, registration.window());
    }

    synchronized void remove(WindowSessionWindowOwnership.Registration registration) {
        windowIds.remove(registration);
        Entry entry = entries.get(registration.window());
        if (entry == null) return; // An ADD may fail before publication registration.
        if (entry.registration != registration) throw new SecurityException("stale window retirement");
        invalidateStaleActivation();
        registry.remove(registration, entry.endpoint);
        rootDecisions.captureAll();
        entries.remove(registration.window());
        if (entry.channelToken != null) {
            channelEntries.remove(entry.channelToken);
        }
        if (entry.boundRoot != null) removeRootBinding(entry);
        driver.retire(entry.endpoint);
        driver.wake();
    }

    /** Joins one current/live original server channel to one authenticated root. */
    @Override public synchronized void bind(DesktopRootRegistry.Registration root,
            android.os.IBinder originalChannelToken, int displayId) {
        if (root == null || originalChannelToken == null)
            throw new DesktopRootRegistry.BindingRejected("missing root or original channel token");
        if (!root.bindingOpen()) throw new DesktopRootRegistry.BindingRejected(
                "desktop root is terminal");
        Entry entry = channelEntries.get(originalChannelToken);
        if (entry == null || !originalChannelToken.equals(entry.channelToken))
            throw new DesktopRootRegistry.BindingRejected("unknown original server channel token");
        if (!entry.live) throw new DesktopRootRegistry.BindingRejected("window is not live");
        if (entry.displayId != displayId) throw new DesktopRootRegistry.BindingRejected(
                "window display mismatch");
        if (!entry.identity.sameAttachment(root.identity))
            throw new DesktopRootRegistry.BindingRejected("window and root attachment mismatch");
        if (entry.boundRoot != null) {
            if (entry.boundRoot == root && entry.boundDisplayId == displayId) return;
            throw new DesktopRootRegistry.BindingRejected(
                    "original channel is bound to another root");
        }
        if (!root.bindingOpen()) throw new DesktopRootRegistry.BindingRejected(
                "desktop root closed during bind");
        WindowFocusRegistry.WindowSpec latest = entry.latestSpec;
        if (latest == null) throw new IllegalStateException("window has no publication snapshot");
        WindowFocusRegistry.WindowSpec paired = paired(latest, root);
        IdentityHashMap<Entry, Boolean> bound = rootBindings.get(root);
        boolean newRootBinding = bound == null;
        if (newRootBinding) {
            bound = new IdentityHashMap<>();
            rootBindings.put(root, bound);
        }
        // Reserve the exact cleanup slot before the potentially allocating
        // registry upsert.  The controller monitor prevents unbind from
        // observing this provisional slot until the immutable fields commit.
        bound.put(entry, Boolean.TRUE);
        try {
            invalidateStaleActivation();
            registry.upsert(paired);
        } catch (RuntimeException | Error failure) {
            bound.remove(entry);
            if (newRootBinding && bound.isEmpty()) rootBindings.remove(root);
            throw failure;
        }
        entry.boundRoot = root;
        entry.boundDisplayId = displayId;
        entry.latestSpec = paired;
        rootDecisions.captureAll();
        driver.wake();
        reconcileBoundRoot(entry);
    }

    /** Removes every exact original entry bound to one terminal root. */
    @Override public synchronized void unbind(DesktopRootRegistry.Registration root) {
        if (root == null) return;
        activationPolicy.retire(root);
        // CLOSED/death has already sealed local input admission. No callback
        // ACK may keep a terminal root alive after its lifetime link is removed.
        rootDecisions.retire(root);
        rootDecisions.captureAll();
        driver.wake();
        IdentityHashMap<Entry, Boolean> bound = rootBindings.get(root);
        if (bound == null) return;
        RuntimeException failure = null;
        boolean changed = false;
        Entry[] snapshot = bound.keySet().toArray(new Entry[bound.size()]);
        for (Entry entry : snapshot) {
            if (entry.boundRoot != root) {
                bound.remove(entry);
                continue;
            }
            try {
                WindowFocusRegistry.WindowSpec unbound = unpaired(entry.latestSpec, root);
                registry.upsert(unbound);
                entry.latestSpec = unbound;
                rootDecisions.captureAll();
                removeRootBinding(entry);
                changed = true;
            } catch (RuntimeException error) {
                if (failure == null) failure = error;
                else failure.addSuppressed(error);
            }
        }
        if (bound.isEmpty()) rootBindings.remove(root);
        if (changed) driver.wake();
        if (failure != null) throw failure;
    }

    private void removeRootBinding(Entry entry) {
        DesktopRootRegistry.Registration root = entry.boundRoot;
        if (root == null) return;
        entry.boundRoot = null;
        IdentityHashMap<Entry, Boolean> bound = rootBindings.get(root);
        if (bound != null) {
            bound.remove(entry);
            if (bound.isEmpty()) rootBindings.remove(root);
        }
    }

    @Override public void factChanged(DesktopRootRegistry.Registration root,
            DesktopRootRegistry.Fact fact) {
        driver.executePolicy(() -> {
            synchronized (WindowPublicationController.this) {
                activationPolicy.reconcile(root, fact);
                rootDecisions.captureAll();
                driver.wake();
            }
        });
    }

    private void reconcileBoundRoot(Entry entry) {
        DesktopRootRegistry.Registration root = entry.boundRoot;
        if (root != null && root.latestFact != null) factChanged(root, root.latestFact);
    }

    private void invalidateStaleActivation() {
        // A loss can commit before the following allocating window mutation.
        // Give that loss progress even if the subsequent operation throws.
        if (activationPolicy.beforeWindowMutation()) {
            rootDecisions.captureAll();
            driver.wake();
        }
    }

    @Override public synchronized void attachFocusDecisions(DesktopRootRegistry.Registration root,
            android.os.IBinder callback) {
        invalidateStaleActivation();
        rootDecisions.attach(root, callback);
    }

    private android.os.IBinder originalToken(WindowFocusRegistry.WindowSpec selected) {
        if (!(selected.windowToken instanceof WindowSessionWindowOwnership.Registration)) return null;
        WindowSessionWindowOwnership.Registration registration =
                (WindowSessionWindowOwnership.Registration) selected.windowToken;
        Entry entry = entries.get(registration.window());
        return entry != null && entry.registration == registration
                && entry.endpoint == selected.channelIncarnation ? entry.channelToken : null;
    }

    private Entry require(WindowSessionWindowOwnership.Registration registration) {
        Entry entry = entries.get(registration.window());
        if (entry == null || entry.registration != registration)
            throw new SecurityException("stale window publication");
        return entry;
    }

    private static boolean isAttached(int type) { return type >= 1000 && type <= 1999; }

    private static WindowFocusRegistry.WindowSpec spec(Entry entry, Rect frame,
            int visibility, int flags) {
        Rect bounds = frame == null ? new Rect() : frame;
        return new WindowFocusRegistry.WindowSpec(entry.registration, entry.endpoint,
                entry.identity.pid(), null, null, entry.displayId, entry.parent,
                entry.parent == null ? WindowFocusRegistry.WindowRole.APPLICATION
                        : WindowFocusRegistry.WindowRole.ATTACHED,
                flags, WindowInputPublisher.inputVisible(bounds, visibility), bounds.left,
                bounds.top, bounds.right, bounds.bottom,
                isAttached(entry.type) ? 10000L + entry.type : 0);
    }

    private static WindowFocusRegistry.WindowSpec paired(WindowFocusRegistry.WindowSpec spec,
            DesktopRootRegistry.Registration root) {
        return new WindowFocusRegistry.WindowSpec(spec.windowToken, spec.channelIncarnation,
                spec.pid, root, root, spec.displayId, spec.parentToken, spec.role, spec.flags,
                spec.visible, spec.left, spec.top, spec.right, spec.bottom, spec.order);
    }

    private static WindowFocusRegistry.WindowSpec unpaired(WindowFocusRegistry.WindowSpec spec,
            DesktopRootRegistry.Registration root) {
        if (spec == null) throw new IllegalStateException("bound window has no snapshot");
        if (spec.rootToken != root || spec.rootIncarnation != root) return spec;
        return new WindowFocusRegistry.WindowSpec(spec.windowToken, spec.channelIncarnation,
                spec.pid, null, null, spec.displayId, spec.parentToken, spec.role, spec.flags,
                spec.visible, spec.left, spec.top, spec.right, spec.bottom, spec.order);
    }
}
