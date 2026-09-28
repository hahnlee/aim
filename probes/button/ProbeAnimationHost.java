package dev.aim.probe;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import android.content.Context;
import android.graphics.Rect;
import android.os.Binder;
import android.os.Handler;
import android.os.Looper;
import android.os.MessageQueue;
import android.view.View;
import android.view.ViewGroup;
import android.view.MotionEvent;
import android.view.ViewTreeObserver;
import android.widget.AbsListView;

/** Creates the hidden RenderNode.AnimationHost without hidden compile stubs. */
public final class ProbeAnimationHost {
    private ProbeAnimationHost() {}
    private static Object sViewRootImpl;
    private static Object sAttachInfo;
    private static Field sPrivateFlags;
    private static Field sRecreateDisplayList;
    private static Field sGroupFlags;

    public static Object create(Class<?> host) {
        try {
            InvocationHandler handler = new InvocationHandler() {
                private final ArrayList<Object> running = new ArrayList<>();

                @Override
                public Object invoke(Object proxy, Method method, Object[] args) {
                    if ("isAttached".equals(method.getName())) {
                        return Boolean.TRUE;
                    }
                    if ("registerAnimatingRenderNode".equals(method.getName()) &&
                            args != null && args.length > 1 && args[1] != null &&
                            !running.contains(args[1])) {
                        // Keep animators reachable like ViewRootImpl; the native
                        // RenderNode owns and advances its native counterpart.
                        running.add(args[1]);
                    }
                    return null;
                }
            };
            return Proxy.newProxyInstance(host.getClassLoader(), new Class<?>[] {host}, handler);
        } catch (Throwable error) {
            error.printStackTrace();
            return null;
        }
    }

    /**
     * Mirrors the first attached ViewRoot traversal for support ViewPager.
     * The APK is unchanged; this only supplies the missing window attachment
     * preparation in the compatibility-owned hierarchy.
     */
    public static void prepareViewPagers(Object root) {
        if (!(root instanceof View)) {
            return;
        }
        // A normal hardware ViewRoot never enables AbsListView's legacy
        // software scrolling cache. This detached owner flattens the hierarchy
        // into one hardware RecordingCanvas, so explicitly retain the same
        // no-cache behavior while child RenderNode ownership is process-local.
        if (root instanceof AbsListView) {
            ((AbsListView) root).setScrollingCacheEnabled(false);
        }
        Class<?> type = root.getClass();
        while (type != null) {
            try {
                Method populate = type.getDeclaredMethod("populate");
                populate.setAccessible(true);
                Field items = type.getDeclaredField("mItems");
                items.setAccessible(true);
                int previousItemCount = ((ArrayList<?>) items.get(root)).size();
                populate.invoke(root);
                int populatedItemCount = ((ArrayList<?>) items.get(root)).size();
                if (populatedItemCount > previousItemCount) {
                    // ViewPager normally requests this traversal from
                    // ViewRootImpl after its window token admits population.
                    // The Metal owner performs that pending traversal on the
                    // next frame so page margins and offsets remain app-owned.
                    ((View) root).requestLayout();
                }
                break;
            } catch (NoSuchMethodException ignored) {
                type = type.getSuperclass();
            } catch (Throwable error) {
                if (System.getenv("AIM_DEBUG_VIEW_TREE") != null) {
                    error.printStackTrace();
                }
                break;
            }
        }
        // Fragment views created by populate() normally inherit AttachInfo
        // through ViewGroup.addView() on an attached parent. The detached
        // Metal owner carries the same object but does not run a WindowManager
        // attach transaction, so propagate it to newly-created children here.
        if (sAttachInfo != null) {
            try {
                Field attachField = View.class.getDeclaredField("mAttachInfo");
                attachField.setAccessible(true);
                installAttachInfo((View) root, attachField, sAttachInfo);
            } catch (Throwable error) {
                if (System.getenv("AIM_DEBUG_VIEW_TREE") != null) {
                    error.printStackTrace();
                }
            }
        }
        // populate() intentionally defers when getWindowToken() is null.  A
        // detached host still needs the same child-order cache for a faithful
        // first draw, so invoke the support ViewPager's private sorting hook
        // and seed its per-child indices exactly as populate() would.
        type = root.getClass();
        while (type != null) {
            try {
                Method sort = type.getDeclaredMethod("sortChildDrawingOrder");
                sort.setAccessible(true);
                sort.invoke(root);
                break;
            } catch (NoSuchMethodException ignored) {
                type = type.getSuperclass();
            } catch (Throwable error) {
                error.printStackTrace();
                break;
            }
        }
        if (root instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) root;
            Object pagerAdapter = null;
            Method pageWidth = null;
            try {
                Method getAdapter = root.getClass().getMethod("getAdapter");
                pagerAdapter = getAdapter.invoke(root);
                if (pagerAdapter != null) {
                    pageWidth = pagerAdapter.getClass().getMethod("getPageWidth", int.class);
                    pageWidth.setAccessible(true);
                }
            } catch (Throwable ignored) {
                // Ordinary ViewGroups do not expose a pager adapter.
            }
            for (int i = 0; i < group.getChildCount(); ++i) {
                View child = group.getChildAt(i);
                try {
                    Object params = child.getLayoutParams();
                    if (params != null) {
                        Field index = params.getClass().getDeclaredField("childIndex");
                        index.setAccessible(true);
                        index.setInt(params, i);
                        try {
                            Field widthFactor = params.getClass().getDeclaredField("widthFactor");
                            widthFactor.setAccessible(true);
                            if (widthFactor.getFloat(params) == 0.0f) {
                                float factor = 1.0f;
                                if (pageWidth != null && pagerAdapter != null) {
                                    try {
                                        factor = ((Float) pageWidth.invoke(pagerAdapter, i)).floatValue();
                                    } catch (Throwable ignored) {
                                        // A non-public adapter implementation may reject reflection;
                                        // the Android default page width is still one full viewport.
                                    }
                                }
                                widthFactor.setFloat(params, factor);
                                Field position = params.getClass().getDeclaredField("position");
                                position.setAccessible(true);
                                position.setInt(params, i);
                                Field needsMeasure = params.getClass().getDeclaredField("needsMeasure");
                                needsMeasure.setAccessible(true);
                                needsMeasure.setBoolean(params, true);
                            }
                        } catch (Throwable ignored) {
                            // Non-pager LayoutParams have no width factor.
                        }
                    }
                } catch (Throwable ignored) {
                    // Non-ViewPager children have ordinary LayoutParams.
                }
                // Some framework versions keep childIndex in a superclass or
                // omit it until the first attached traversal. Seed the page
                // sizing fields independently so a missing ordering field
                // cannot leave a whole ViewPager at width zero.
                try {
                    Object params = child.getLayoutParams();
                    Field widthFactor = params.getClass().getDeclaredField("widthFactor");
                    widthFactor.setAccessible(true);
                    if (widthFactor.getFloat(params) == 0.0f) {
                        float factor = 1.0f;
                        if (pageWidth != null && pagerAdapter != null) {
                            try {
                                factor = ((Float) pageWidth.invoke(pagerAdapter, i)).floatValue();
                            } catch (Throwable ignored) {
                                // Default to one full viewport when adapter reflection is blocked.
                            }
                        }
                        widthFactor.setFloat(params, factor);
                        Field position = params.getClass().getDeclaredField("position");
                        position.setAccessible(true);
                        position.setInt(params, i);
                        Field needsMeasure = params.getClass().getDeclaredField("needsMeasure");
                        needsMeasure.setAccessible(true);
                        needsMeasure.setBoolean(params, true);
                    }
                } catch (Throwable ignored) {
                    // Not a ViewPager page; leave its ordinary params untouched.
                }
                prepareViewPagers(child);
            }
            // A detached ViewPager can have its adapter's ItemInfo list empty
            // when the first Android traversal ran before it had a client
            // width. In that case the normal onLayout loop intentionally skips
            // pages. Once the pager itself has real bounds, lay out each
            // existing page exactly like a full-width ViewPager item so its
            // GridLayout descendants receive finite button bounds.
            if (root.getClass().getName().contains("ViewPager") &&
                    ((View) root).getWindowToken() == null &&
                    group.getMeasuredWidth() > 0 && group.getMeasuredHeight() > 0) {
                int viewportWidth = group.getMeasuredWidth();
                int pageHeight = group.getMeasuredHeight();
                int heightSpec = View.MeasureSpec.makeMeasureSpec(pageHeight, View.MeasureSpec.EXACTLY);
                int pageMargin = 0;
                try {
                    Method getPageMargin = root.getClass().getMethod("getPageMargin");
                    pageMargin = ((Integer) getPageMargin.invoke(root)).intValue();
                } catch (Throwable ignored) {
                    // Zero is the ViewPager default.
                }
                int pageLeft = 0;
                for (int i = 0; i < group.getChildCount(); ++i) {
                    View child = group.getChildAt(i);
                    if (child.getVisibility() != View.GONE) {
                        float factor = 1.0f;
                        if (pageWidth != null && pagerAdapter != null) {
                            try {
                                factor = ((Float) pageWidth.invoke(pagerAdapter, i)).floatValue();
                            } catch (Throwable ignored) {
                                // One full viewport is the adapter default.
                            }
                        }
                        int childWidth = Math.round(viewportWidth * factor);
                        int widthSpec = View.MeasureSpec.makeMeasureSpec(
                                childWidth, View.MeasureSpec.EXACTLY);
                        child.measure(widthSpec, heightSpec);
                        child.layout(pageLeft, 0, pageLeft + childWidth, pageHeight);
                        pageLeft += childWidth + pageMargin;
                    }
                }
            }
        }
    }

    /**
     * Runs the pre-draw phase of a normal ViewRoot traversal. TextView uses
     * this phase to resolve deferred scrolling for single-line, horizontally
     * scrolling layouts (whose internal width is VERY_WIDE). Without it,
     * centered glyphs remain positioned around x=524288 and are clipped.
     */
    public static boolean dispatchPreDraw(Object root) {
        if (!(root instanceof View)) return false;
        ViewTreeObserver observer = ((View) root).getViewTreeObserver();
        return observer != null && observer.isAlive() && observer.dispatchOnPreDraw();
    }

    /**
     * Notifies listeners at the same point as ViewRootImpl, immediately before
     * the hierarchy is drawn. Chromium uses the first onDraw callback as the
     * gate for starting its native browser process.
     */
    public static void dispatchOnDraw(Object root) {
        if (!(root instanceof View)) return;
        ViewTreeObserver observer = ((View) root).getViewTreeObserver();
        if (observer != null && observer.isAlive()) observer.dispatchOnDraw();
    }

    /**
     * Consumes the synchronization barrier installed by ViewRootImpl when the
     * host performs that pending traversal on its behalf.  Leaving the barrier
     * queued would indefinitely block ordinary Handler work behind it.
     */
    public static void beginHostTraversal(Object root) {
        if (!(root instanceof View)) return;
        try {
            Method getViewRoot = View.class.getDeclaredMethod("getViewRootImpl");
            getViewRoot.setAccessible(true);
            Object viewRoot = getViewRoot.invoke(root);
            if (viewRoot == null) return;
            Field scheduled = viewRoot.getClass().getDeclaredField("mTraversalScheduled");
            Field barrier = viewRoot.getClass().getDeclaredField("mTraversalBarrier");
            scheduled.setAccessible(true);
            barrier.setAccessible(true);
            if (!scheduled.getBoolean(viewRoot)) return;
            int token = barrier.getInt(viewRoot);
            Handler handler = ((View) root).getHandler();
            if (handler == null) return;
            MessageQueue queue = handler.getLooper().getQueue();
            Method removeBarrier = MessageQueue.class.getDeclaredMethod(
                    "removeSyncBarrier", int.class);
            removeBarrier.setAccessible(true);
            removeBarrier.invoke(queue, Integer.valueOf(token));
            scheduled.setBoolean(viewRoot, false);
            barrier.setInt(viewRoot, -1);
        } catch (Throwable ignored) {
            // Detached probe roots have no WindowManager-owned ViewRootImpl.
        }
    }

    /** Mark every child dirty before replacing the flattened GPU display list. */
    public static void invalidateViewTree(Object root) {
        if (!(root instanceof View)) return;
        View view = (View) root;
        try {
            if (sPrivateFlags == null) {
                sPrivateFlags = View.class.getDeclaredField("mPrivateFlags");
                sPrivateFlags.setAccessible(true);
                sRecreateDisplayList = View.class.getDeclaredField("mRecreateDisplayList");
                sRecreateDisplayList.setAccessible(true);
                sGroupFlags = ViewGroup.class.getDeclaredField("mGroupFlags");
                sGroupFlags.setAccessible(true);
            }
            int flags = sPrivateFlags.getInt(view);
            // PFLAG_DIRTY_MASK=0x00600000, PFLAG_DIRTY=0x00200000,
            // PFLAG_INVALIDATED=0x80000000 in the pinned Android 16 framework.
            sPrivateFlags.setInt(view,
                    (flags & ~0x00600000) | 0x00200000 | 0x80000000);
            sRecreateDisplayList.setBoolean(view, true);
            if (view instanceof ViewGroup) {
                int groupFlags = sGroupFlags.getInt(view);
                sGroupFlags.setInt(view, groupFlags | 0x00000004);
            }
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Unable to invalidate flattened View tree", error);
        }
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int index = 0; index < group.getChildCount(); ++index) {
                invalidateViewTree(group.getChildAt(index));
            }
        }
    }

    /** Attach the real hierarchy bookkeeping without creating a second window.
     * The Metal surface remains owned by the host; this only supplies the
     * AttachInfo that Android View.draw() normally receives from ViewRootImpl.
     */
    public static boolean attachHardwareHierarchy(Object root, Object contextObject) {
        if (sViewRootImpl != null) return true;
        if (!(root instanceof View)) return false;
        try {
            Context context = contextObject instanceof Context
                    ? (Context) contextObject : ((View) root).getContext();
            if (context == null) {
                Class<?> probeContext = Class.forName("dev.aim.probe.ProbeContext");
                Constructor<?> probeConstructor = probeContext.getConstructor(
                        Class.forName("android.content.res.Resources"),
                        Class.forName("android.content.pm.PackageManager"),
                        String.class);
                context = (Context) probeConstructor.newInstance(
                        ((View) root).getResources(), null, "com.android.calculator2");
            }
            if (context == null) return false;
            // Build the same AttachInfo object directly: View's hardware
            // checks, invalidation, and RenderNode recording observe the
            // normal Android fields, while the host's CAMetalLayer remains
            // the sole window owner.
            Class<?> displayType = Class.forName("android.view.Display");
            Class<?> iWindowSession = Class.forName("android.view.IWindowSession");
            Class<?> iWindow = Class.forName("android.view.IWindow");
            Binder windowToken = new Binder();
            InvocationHandler windowHandler = (proxy, method, args) -> {
                if ("asBinder".equals(method.getName())) return windowToken;
                return defaultValue(method.getReturnType());
            };
            Object window = Proxy.newProxyInstance(
                    iWindow.getClassLoader(), new Class<?>[] {iWindow}, windowHandler);
            Object session = Proxy.newProxyInstance(
                    iWindowSession.getClassLoader(), new Class<?>[] {iWindowSession},
                    windowHandler);
            Class<?> attachType = Class.forName("android.view.View$AttachInfo");
            Class<?> callbacks = Class.forName("android.view.View$AttachInfo$Callbacks");
            // The real ViewRootImpl owns the invalidation/choreographer bridge
            // used by TextView.append() and postInvalidateOnAnimation().  The
            // detached host still creates it with the normal Android
            // constructor; only the WindowManager add/traversal transaction
            // remains omitted because CAMetalLayer is the window owner.
            Class<?> viewRootType = Class.forName("android.view.ViewRootImpl");
            Class<?> displayGlobalType = Class.forName(
                    "android.hardware.display.DisplayManagerGlobal");
            Method displayInstance = displayGlobalType.getDeclaredMethod("getInstance");
            displayInstance.setAccessible(true);
            Object displayGlobal = displayInstance.invoke(null);
            Method realDisplay = displayGlobalType.getDeclaredMethod(
                    "getRealDisplay", int.class);
            realDisplay.setAccessible(true);
            Object display = realDisplay.invoke(displayGlobal, Integer.valueOf(0));
            if (display == null) return false;
            Constructor<?> viewRootConstructor = viewRootType.getDeclaredConstructor(
                    Context.class, displayType);
            viewRootConstructor.setAccessible(true);
            Object viewRoot = viewRootConstructor.newInstance(context, display);
            // PopupWindow asks the owning ViewRoot for the usable display
            // frame before it builds dropdown LayoutParams. The detached root
            // has not received a WMS relayout, so seed the physical-pixel frame
            // exposed by the process-local display service. Resources keep the
            // corresponding 360x640 dp configuration through density.
            Field tmpFramesField = viewRootType.getDeclaredField("mTmpFrames");
            tmpFramesField.setAccessible(true);
            Object tmpFrames = tmpFramesField.get(viewRoot);
            Field displayFrameField = tmpFrames.getClass().getField("displayFrame");
            int displayScale = "2".equals(System.getenv("AIM_WINDOW_SCALE"))
                    ? 2 : 1;
            ((Rect) displayFrameField.get(tmpFrames)).set(
                    0, 0, 360 * displayScale, 640 * displayScale);
            Constructor<?> attachConstructor = attachType.getDeclaredConstructor(
                    iWindowSession, iWindow, displayType,
                    viewRootType, Handler.class,
                    callbacks, Context.class);
            attachConstructor.setAccessible(true);
            Object attachInfo = attachConstructor.newInstance(
                    session, window, display, viewRoot,
                    new Handler(Looper.getMainLooper()), null, context);
            Field hardwareField = attachInfo.getClass().getDeclaredField(
                    "mHardwareAccelerated");
            hardwareField.setAccessible(true);
            // The detached owner still records into the same hardware
            // RecordingCanvas that a real ViewRoot uses. Keep AttachInfo's
            // hardware bit truthful as well: support-library pagers and other
            // framework widgets select software/bitmap fallbacks when this flag
            // is false, leaving their content black or absent.
            hardwareField.setBoolean(attachInfo, true);
            Field requestedField = attachInfo.getClass().getDeclaredField(
                    "mHardwareAccelerationRequested");
            requestedField.setAccessible(true);
            requestedField.setBoolean(attachInfo, true);
            // Do not call dispatchAttachedToWindow here. That method assumes
            // a live ViewRootImpl (IME focus, window callbacks and traversal
            // callbacks) and is not safe for this windowless Metal owner. The
            // detached GPU traversal only needs the same AttachInfo pointer so
            // ViewGroup.dispatchDraw and hardware canvas checks retain their
            // normal Android decisions. Propagate it explicitly through the
            // hierarchy; no Binder/window lifecycle is synthesized.
            Field attachField = View.class.getDeclaredField("mAttachInfo");
            attachField.setAccessible(true);
            installAttachInfo((View) root, attachField, attachInfo);
            // Give the detached ViewRootImpl the same root reference that a
            // real WindowManager.addView() would install. This enables the
            // app-side InputStage chain without creating a second NSWindow.
            Field rootField = viewRootType.getDeclaredField("mView");
            rootField.setAccessible(true);
            rootField.set(viewRoot, root);
            sAttachInfo = attachInfo;
            sViewRootImpl = viewRoot;
            return true;
        } catch (Throwable error) {
            error.printStackTrace();
            return false;
        }
    }

    /** Enqueue one framework MotionEvent through ViewRootImpl's app-side
     * input stages. Returns false when the detached root is unavailable so
     * the native bridge can use DecorView dispatch as its bounded gate. */
    public static boolean enqueueInputEvent(Object event) {
        if (sViewRootImpl == null || event == null) return false;
        try {
            Class<?> inputEvent = Class.forName("android.view.InputEvent");
            Method enqueue = sViewRootImpl.getClass().getDeclaredMethod(
                    "enqueueInputEvent", inputEvent,
                    Class.forName("android.view.InputEventReceiver"),
                    int.class, boolean.class);
            enqueue.setAccessible(true);
            enqueue.invoke(sViewRootImpl, event, null, Integer.valueOf(0), Boolean.TRUE);
            return true;
        } catch (Throwable error) {
            if (System.getenv("AIM_DEBUG_INPUT_LATENCY") != null) {
                error.printStackTrace();
            }
            return false;
        }
    }

    /**
     * Exercise the Android MotionEvent history contract against the native
     * registrar. This is opt-in diagnostic coverage; production APK input
     * never allocates a probe event or takes this branch.
     */
    public static int motionEventArchiveProbe() {
        MotionEvent event = null;
        MotionEvent copy = null;
        MotionEvent noHistory = null;
        try {
            event = MotionEvent.obtain(10L, 10L, MotionEvent.ACTION_MOVE,
                    1.0f, 2.0f, 0);
            event.addBatch(20L, 3.0f, 4.0f, 1.0f, 1.0f, 0);
            if (event.getHistorySize() != 1
                    || event.getHistoricalEventTime(0) != 10L
                    || event.getHistoricalX(0) != 1.0f
                    || event.getHistoricalY(0) != 2.0f
                    || event.getX() != 3.0f
                    || event.getY() != 4.0f) {
                return 1;
            }
            copy = MotionEvent.obtain(event);
            if (copy.getHistorySize() != 1
                    || copy.getHistoricalX(0) != 1.0f
                    || copy.getHistoricalY(0) != 2.0f) {
                return 2;
            }
            noHistory = MotionEvent.obtainNoHistory(event);
            if (noHistory.getHistorySize() != 0
                    || noHistory.getX() != 3.0f
                    || noHistory.getY() != 4.0f) {
                return 3;
            }
            return 0;
        } catch (Throwable error) {
            if (System.getenv("AIM_DEBUG_INPUT_LATENCY") != null) {
                error.printStackTrace();
            }
            return 3;
        } finally {
            if (noHistory != null) noHistory.recycle();
            if (copy != null) copy.recycle();
            if (event != null) event.recycle();
        }
    }

    private static Object defaultValue(Class<?> type) {
        if (!type.isPrimitive()) return null;
        if (type == boolean.class) return Boolean.FALSE;
        if (type == byte.class) return Byte.valueOf((byte) 0);
        if (type == short.class) return Short.valueOf((short) 0);
        if (type == char.class) return Character.valueOf((char) 0);
        if (type == int.class) return Integer.valueOf(0);
        if (type == long.class) return Long.valueOf(0L);
        if (type == float.class) return Float.valueOf(0.0f);
        if (type == double.class) return Double.valueOf(0.0d);
        return null;
    }

    private static void clearFocusTree(View view) {
        view.clearFocus();
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); ++i) {
                clearFocusTree(group.getChildAt(i));
            }
        }
    }

    private static void installAttachInfo(View view, Field attachField,
            Object attachInfo) throws IllegalAccessException {
        if (view == null) return;
        attachField.set(view, attachInfo);
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); ++i) {
                installAttachInfo(group.getChildAt(i), attachField, attachInfo);
            }
        }
    }
}
