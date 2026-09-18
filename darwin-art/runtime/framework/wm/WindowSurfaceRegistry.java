package dev.darwinart.runtime.wm;

import android.content.res.Configuration;
import android.content.res.Resources;
import android.graphics.Rect;
import android.os.IBinder;
import android.os.Parcelable;
import android.view.Gravity;
import android.view.SurfaceControl;
import android.view.WindowManager;
import dev.darwinart.runtime.display.BuiltInDisplayConfiguration;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.HashMap;

/** Owns WMS-side SurfaceControl layers and their Android relayout state. */
final class WindowSurfaceRegistry {
    private final DesktopWindowMetadataRegistry metadata;
    private final HashMap<IBinder, SurfaceControl> surfaces = new HashMap<>();
    private final HashMap<IBinder, int[]> sizes = new HashMap<>();
    private final HashMap<IBinder, WindowManager.LayoutParams> attributes = new HashMap<>();
    private final HashMap<IBinder, Rect> frames = new HashMap<>();

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

    WindowSurfaceRegistry(DesktopWindowMetadataRegistry registry) {
        metadata = registry;
    }

    synchronized void add(int pid, IBinder window, WindowManager.LayoutParams attrs,
            int visibility) {
        if (window == null) throw new IllegalArgumentException("add requires IWindow");
        if (attrs != null) attributes.put(window, copyOf(attrs));
        metadata.update(pid, window, attrs, visibility);
    }

    synchronized RelayoutPublication relayout(int pid, IBinder window, WindowManager.LayoutParams attrs,
            int requestedWidth, int requestedHeight, int visibility, int syncSequenceId) {
        if (window == null) throw new IllegalArgumentException("relayout requires IWindow");
        if (attrs != null) attributes.put(window, copyOf(attrs));
        WindowManager.LayoutParams layout = attributes.get(window);
        metadata.update(pid, window, layout, visibility);
        int requestedLayoutWidth = layout != null ? layout.width : 0;
        int requestedLayoutHeight = layout != null ? layout.height : 0;
        int width = requestedWidth > 0 ? requestedWidth
                : requestedLayoutWidth > 0 ? requestedLayoutWidth
                : BuiltInDisplayConfiguration.WIDTH_PIXELS;
        int height = requestedHeight > 0 ? requestedHeight
                : requestedLayoutHeight > 0 ? requestedLayoutHeight
                : BuiltInDisplayConfiguration.HEIGHT_PIXELS;
        width = Math.min(width, BuiltInDisplayConfiguration.WIDTH_PIXELS);
        height = Math.min(height, BuiltInDisplayConfiguration.HEIGHT_PIXELS);
        Rect frame = layoutFrame(layout, width, height);
        if (System.getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != null) {
            android.util.Log.i("DarwinWindowSurface", "relayout type="
                    + (layout == null ? 0 : layout.type) + " gravity=0x"
                    + Integer.toHexString(layout == null ? 0 : layout.gravity)
                    + " xy=" + (layout == null ? 0 : layout.x) + ","
                    + (layout == null ? 0 : layout.y) + " requested="
                    + requestedWidth + "x" + requestedHeight + " surface="
                    + width + "x" + height + " frame=" + frame);
        }
        frames.put(window, frame);
        SurfaceControl producer = surfaces.get(window);
        int[] oldSize = sizes.get(window);
        if (producer == null || !producer.isValid() || oldSize == null
                || oldSize[0] != width || oldSize[1] != height) {
            if (producer != null && producer.isValid()) producer.release();
            producer = new SurfaceControl.Builder()
                    .setName("Darwin ART ViewRoot")
                    .setBufferSize(width, height)
                    .build();
            surfaces.put(window, producer);
            sizes.put(window, new int[] {width, height});
        }
        try (SurfaceControl.Transaction transaction = new SurfaceControl.Transaction()) {
            int layer = layout != null && layout.type >= WindowManager.LayoutParams.FIRST_SUB_WINDOW
                    ? 10_000 + layout.type : 0;
            transaction.setPosition(producer, frame.left, frame.top)
                    .setLayer(producer, layer).apply();
        }
        return new RelayoutPublication(createRelayoutResult(producer, frame, syncSequenceId),
                frame, visibility, layout);
    }

    synchronized WindowManager.LayoutParams effectiveAttributes(IBinder window,
            WindowManager.LayoutParams requested) {
        WindowManager.LayoutParams effective = requested == null ? attributes.get(window) : requested;
        return effective == null ? null : copyOf(effective);
    }

    synchronized void remove(int pid, IBinder window) {
        metadata.remove(pid, window);
        SurfaceControl surface = surfaces.get(window);
        if (surface != null && surface.isValid()) {
            try (SurfaceControl.Transaction transaction = new SurfaceControl.Transaction()) {
                transaction.reparent(surface, null).apply();
            }
            surface.release();
        }
        // Do not lose the original layer if transaction/release throws.
        surfaces.remove(window);
        sizes.remove(window);
        attributes.remove(window);
        frames.remove(window);
    }

    synchronized Rect frame(IBinder window) {
        Rect frame = frames.get(window);
        return frame == null ? null : new Rect(frame);
    }

    private static Rect layoutFrame(WindowManager.LayoutParams attrs, int width, int height) {
        Rect display = new Rect(0, 0, BuiltInDisplayConfiguration.WIDTH_PIXELS,
                BuiltInDisplayConfiguration.HEIGHT_PIXELS);
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
        return copy;
    }

    private static Parcelable createRelayoutResult(
            SurfaceControl producer, Rect frame, int syncSequenceId) {
        try {
            Class<?> resultClass = Class.forName("android.view.WindowRelayoutResult");
            Object result = resultClass.getDeclaredConstructor().newInstance();
            Object frames = resultClass.getField("frames").get(result);
            setRect(frames, "frame", frame);
            setRect(frames, "displayFrame", BuiltInDisplayConfiguration.WIDTH_PIXELS,
                    BuiltInDisplayConfiguration.HEIGHT_PIXELS);
            setRect(frames, "parentFrame", BuiltInDisplayConfiguration.WIDTH_PIXELS,
                    BuiltInDisplayConfiguration.HEIGHT_PIXELS);

            Configuration global = BuiltInDisplayConfiguration.configuration(
                    Resources.getSystem().getConfiguration());
            if (System.getenv("DARWIN_ART_DEBUG_DISPLAY_CONFIGURATION") != null) {
                android.util.Log.i("WindowSurfaceRegistry",
                        "relayout system configuration=" + global
                        + " metrics=" + Resources.getSystem().getDisplayMetrics()
                        + " window=" + frame);
            }
            Object mergedConfiguration = resultClass.getField("mergedConfiguration").get(result);
            mergedConfiguration.getClass().getMethod(
                    "setConfiguration", Configuration.class, Configuration.class)
                    .invoke(mergedConfiguration, global, new Configuration());

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

    private static void setRect(Object frames, String fieldName, int width, int height)
            throws ReflectiveOperationException {
        Field field = frames.getClass().getField(fieldName);
        ((Rect) field.get(frames)).set(0, 0, width, height);
    }

    private static void setRect(Object frames, String fieldName, Rect value)
            throws ReflectiveOperationException {
        Field field = frames.getClass().getField(fieldName);
        ((Rect) field.get(frames)).set(value);
    }
}
