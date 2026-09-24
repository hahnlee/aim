package dev.darwinart.runtime.wm;

import android.content.res.Configuration;
import android.content.res.Resources;
import android.graphics.Rect;
import android.os.IBinder;
import android.os.Parcelable;
import android.view.Gravity;
import android.view.SurfaceControl;
import android.view.ViewGroup;
import android.view.WindowManager;
import dev.darwinart.runtime.display.DisplayGeometry;
import dev.darwinart.runtime.display.TaskDisplayRegistry;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Owns WMS-side SurfaceControl layers and their Android relayout state.
 *
 * <p>One registry serves every window session so a task geometry change can
 * lay out all windows of that task from one snapshot. Frames are computed
 * against the owning task's current bounds; MATCH_PARENT always follows the
 * task, never the previously allocated buffer.</p>
 */
final class WindowSurfaceRegistry {
    private static final class WindowRecord {
        final int pid;
        SurfaceControl surface;
        int surfaceWidth;
        int surfaceHeight;
        WindowManager.LayoutParams attributes;
        Rect frame;
        int requestedWidth;
        int requestedHeight;

        WindowRecord(int pid) {
            this.pid = pid;
        }
    }

    /** One window's frame for a task geometry revision. */
    static final class TaskWindowFrame {
        final IBinder window;
        final IBinder attachedToken;
        final int type;
        final Rect frame;

        TaskWindowFrame(IBinder window, IBinder attachedToken, int type, Rect frame) {
            this.window = window;
            this.attachedToken = attachedToken;
            this.type = type;
            this.frame = new Rect(frame);
        }
    }

    private final DesktopWindowMetadataRegistry metadata;
    private final TaskDisplayRegistry displays;
    private final HashMap<IBinder, WindowRecord> windows = new HashMap<>();

    static final class RelayoutPublication {
        final Parcelable result;
        final Rect frame;
        final int viewVisibility;
        final WindowManager.LayoutParams effectiveAttributes;

        RelayoutPublication(Parcelable result, Rect frame, int viewVisibility,
                WindowManager.LayoutParams effectiveAttributes) {
            this.result = result;
            this.frame = new Rect(frame);
            this.viewVisibility = viewVisibility;
            this.effectiveAttributes = effectiveAttributes == null ? null : copyOf(effectiveAttributes);
        }
    }

    WindowSurfaceRegistry(DesktopWindowMetadataRegistry registry, TaskDisplayRegistry displays) {
        if (registry == null || displays == null) throw new NullPointerException();
        metadata = registry;
        this.displays = displays;
    }

    /** Task bounds reported by addToDisplay for a new window. */
    Rect taskBounds(int pid) {
        return displays.geometry(pid).bounds();
    }

    synchronized void add(int pid, IBinder window, WindowManager.LayoutParams attrs,
            int visibility) {
        if (window == null) throw new IllegalArgumentException("add requires IWindow");
        WindowRecord record = windows.get(window);
        if (record == null) {
            record = new WindowRecord(pid);
            windows.put(window, record);
        }
        if (attrs != null) record.attributes = copyOf(attrs);
        metadata.update(pid, window, attrs, visibility);
    }

    synchronized RelayoutPublication relayout(int pid, IBinder window, WindowManager.LayoutParams attrs,
            int requestedWidth, int requestedHeight, int visibility, int syncSequenceId) {
        if (window == null) throw new IllegalArgumentException("relayout requires IWindow");
        WindowRecord record = windows.get(window);
        if (record == null || record.pid != pid) {
            throw new IllegalStateException("relayout of a window this task does not own");
        }
        if (attrs != null) record.attributes = copyOf(attrs);
        WindowManager.LayoutParams layout = record.attributes;
        metadata.update(pid, window, layout, visibility);
        record.requestedWidth = requestedWidth;
        record.requestedHeight = requestedHeight;
        // One snapshot for frame, buffer size and merged configuration.
        DisplayGeometry geometry = displays.geometry(pid);
        Rect frame = frameFor(record, geometry);
        int width = frame.width();
        int height = frame.height();
        if (System.getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != null
                || System.getenv("DARWIN_ART_DEBUG_TASK_GEOMETRY") != null) {
            android.util.Log.i("DarwinWindowSurface", "relayout type="
                    + (layout == null ? 0 : layout.type) + " gravity=0x"
                    + Integer.toHexString(layout == null ? 0 : layout.gravity)
                    + " xy=" + (layout == null ? 0 : layout.x) + ","
                    + (layout == null ? 0 : layout.y) + " requested="
                    + requestedWidth + "x" + requestedHeight + " surface="
                    + width + "x" + height + " frame=" + frame + " task=" + geometry);
        }
        record.frame = frame;
        // WindowLayout.computeSurfaceSize: the client buffer covers the frame
        // plus surface insets (elevation shadows); WMS offsets the layer so the
        // frame, not the shadow margin, lands at the laid-out position.
        Rect insets = surfaceInsets(layout);
        width += insets.left + insets.right;
        height += insets.top + insets.bottom;
        // WMS keeps one layer for the window's lifetime, as AOSP's
        // WindowSurfaceController does: a resize changes only the BLAST buffer
        // size the client requests. Replacing the layer would make ViewRootImpl
        // rebuild its surface and put child SurfaceViews through surface loss.
        SurfaceControl producer = record.surface;
        if (producer == null || !producer.isValid()) {
            producer = new SurfaceControl.Builder()
                    .setName("Darwin ART ViewRoot")
                    .setBufferSize(width, height)
                    .build();
            record.surface = producer;
            record.surfaceWidth = width;
            record.surfaceHeight = height;
        }
        try (SurfaceControl.Transaction transaction = new SurfaceControl.Transaction()) {
            int layer = layout != null && layout.type >= WindowManager.LayoutParams.FIRST_SUB_WINDOW
                    ? 10_000 + layout.type : 0;
            transaction.setPosition(producer, frame.left - insets.left, frame.top - insets.top)
                    .setLayer(producer, layer).apply();
        }
        return new RelayoutPublication(
                createRelayoutResult(producer, frame, geometry, syncSequenceId),
                frame, visibility, layout);
    }

    /**
     * Lays out every window of one task against {@code geometry}. The frames
     * are published with the matching resize transaction; the client's
     * following relayout recomputes the same frames from the same snapshot.
     */
    synchronized List<TaskWindowFrame> taskFrames(int pid, DisplayGeometry geometry) {
        ArrayList<TaskWindowFrame> result = new ArrayList<>();
        for (Map.Entry<IBinder, WindowRecord> entry : windows.entrySet()) {
            WindowRecord record = entry.getValue();
            if (record.pid != pid || record.frame == null) continue;
            WindowManager.LayoutParams layout = record.attributes;
            result.add(new TaskWindowFrame(entry.getKey(), layout == null ? null : layout.token,
                    layout == null ? 0 : layout.type, frameFor(record, geometry)));
        }
        return result;
    }

    synchronized WindowManager.LayoutParams effectiveAttributes(IBinder window,
            WindowManager.LayoutParams requested) {
        WindowRecord record = windows.get(window);
        WindowManager.LayoutParams effective = requested == null
                ? record == null ? null : record.attributes : requested;
        return effective == null ? null : copyOf(effective);
    }

    synchronized void remove(int pid, IBinder window) {
        metadata.remove(pid, window);
        WindowRecord record = windows.get(window);
        if (record != null && record.pid != pid) return;
        SurfaceControl surface = record == null ? null : record.surface;
        if (surface != null && surface.isValid()) {
            try (SurfaceControl.Transaction transaction = new SurfaceControl.Transaction()) {
                transaction.reparent(surface, null).apply();
            }
            surface.release();
        }
        // Do not lose the original layer if transaction/release throws.
        windows.remove(window);
    }

    synchronized Rect frame(IBinder window) {
        WindowRecord record = windows.get(window);
        return record == null || record.frame == null ? null : new Rect(record.frame);
    }

    private static final Field SURFACE_INSETS = surfaceInsetsField();

    private static Rect surfaceInsets(WindowManager.LayoutParams layout) {
        if (layout == null) return new Rect();
        try {
            Rect insets = (Rect) SURFACE_INSETS.get(layout);
            return insets == null ? new Rect() : new Rect(insets);
        } catch (IllegalAccessException error) {
            throw new IllegalStateException("cannot read WindowManager surfaceInsets", error);
        }
    }

    private static Field surfaceInsetsField() {
        try {
            Field field = WindowManager.LayoutParams.class.getDeclaredField("surfaceInsets");
            field.setAccessible(true);
            return field;
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static Rect frameFor(WindowRecord record, DisplayGeometry geometry) {
        WindowManager.LayoutParams layout = record.attributes;
        int taskWidth = geometry.widthPixels;
        int taskHeight = geometry.heightPixels;
        int width = extent(layout == null ? ViewGroup.LayoutParams.MATCH_PARENT : layout.width,
                record.requestedWidth, taskWidth);
        int height = extent(layout == null ? ViewGroup.LayoutParams.MATCH_PARENT : layout.height,
                record.requestedHeight, taskHeight);
        return layoutFrame(layout, width, height, geometry.bounds());
    }

    private static int extent(int layoutSize, int requested, int task) {
        int size;
        if (layoutSize == ViewGroup.LayoutParams.MATCH_PARENT) {
            size = task;
        } else if (requested > 0) {
            size = requested;
        } else if (layoutSize > 0) {
            size = layoutSize;
        } else {
            size = task;
        }
        return Math.max(1, Math.min(size, task));
    }

    private static Rect layoutFrame(WindowManager.LayoutParams attrs, int width, int height,
            Rect display) {
        Rect frame = new Rect();
        int gravity = attrs == null || attrs.gravity == 0
                ? Gravity.START | Gravity.TOP : attrs.gravity;
        int x = attrs == null ? 0 : attrs.x;
        int y = attrs == null ? 0 : attrs.y;
        Gravity.apply(gravity, width, height, display, x, y, frame);
        Gravity.applyDisplay(gravity, display, frame);
        return frame;
    }

    private static WindowManager.LayoutParams copyOf(WindowManager.LayoutParams source) {
        WindowManager.LayoutParams copy = new WindowManager.LayoutParams();
        copy.copyFrom(source);
        // copyFrom does not transfer the attachment token; frame dispatch
        // needs it to associate windows with their Activity or parent window.
        copy.token = source.token;
        return copy;
    }

    private static Parcelable createRelayoutResult(SurfaceControl producer, Rect frame,
            DisplayGeometry geometry, int syncSequenceId) {
        try {
            Class<?> resultClass = Class.forName("android.view.WindowRelayoutResult");
            Object result = resultClass.getDeclaredConstructor().newInstance();
            Object frames = resultClass.getField("frames").get(result);
            setRect(frames, "frame", frame);
            setRect(frames, "displayFrame", geometry.bounds());
            setRect(frames, "parentFrame", geometry.bounds());
            // Relayout and WindowStateResizeItem frames of one task revision
            // share a sequence so ViewRootImpl discards only older frames.
            frames.getClass().getField("seq").setInt(frames, (int) geometry.revision);

            Configuration global = geometry.configuration(
                    Resources.getSystem().getConfiguration());
            if (System.getenv("DARWIN_ART_DEBUG_DISPLAY_CONFIGURATION") != null) {
                android.util.Log.i("WindowSurfaceRegistry",
                        "relayout task configuration=" + global + " window=" + frame);
            }
            Object mergedConfiguration = resultClass.getField("mergedConfiguration").get(result);
            mergedConfiguration.getClass().getMethod(
                    "setConfiguration", Configuration.class, Configuration.class)
                    .invoke(mergedConfiguration, global, geometry.overrideConfiguration());

            SurfaceControl output = (SurfaceControl) resultClass.getField("surfaceControl")
                    .get(result);
            Method copyFrom = SurfaceControl.class.getDeclaredMethod(
                    "copyFrom", SurfaceControl.class, String.class);
            copyFrom.setAccessible(true);
            copyFrom.invoke(output, producer, "WindowSurfaceRegistry.relayout");
            resultClass.getField("syncSeqId").setInt(result, syncSequenceId);
            return (Parcelable) result;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android WindowRelayoutResult ABI mismatch", error);
        }
    }

    private static void setRect(Object frames, String fieldName, Rect value)
            throws ReflectiveOperationException {
        Field field = frames.getClass().getField(fieldName);
        ((Rect) field.get(frames)).set(value);
    }
}
