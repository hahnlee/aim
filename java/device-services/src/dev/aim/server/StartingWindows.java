package dev.aim.server;

import android.app.ActivityManager.RunningTaskInfo;
import android.app.ActivityTaskManager;
import android.app.WindowConfiguration;
import android.content.Context;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.content.res.TypedArray;
import android.graphics.Color;
import android.graphics.PixelFormat;
import android.graphics.drawable.AdaptiveIconDrawable;
import android.graphics.drawable.Animatable;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.hardware.display.DisplayManager;
import android.os.Handler;
import android.os.RemoteCallback;
import android.os.UserHandle;
import android.util.Slog;
import android.view.ContextThemeWrapper;
import android.view.Display;
import android.view.WindowManager;
import android.view.WindowManagerGlobal;
import android.widget.FrameLayout;
import android.window.SplashScreenView;
import android.window.SplashScreenView.SplashScreenViewParcelable;
import android.window.StartingWindowInfo;
import android.window.StartingWindowRemovalInfo;

import java.util.HashMap;
import java.util.Map;

/**
 * The window shell's starting windows (docs/task-organizer.md, "Starting
 * windows"): with a task organizer registered, WindowManager asks it for a
 * task's starting window instead of drawing one. A new task or a cold start
 * gets a splash screen from the activity's theme (its splash screen
 * background, icon and branding image), which an app can take over with
 * SplashScreen.setOnExitAnimationListener; a task switch to a running
 * activity gets none, as without a snapshot. As WMShell's
 * StartingSurfaceDrawer does (an animated icon plays, SplashIcons), but
 * without its reveal animation: the Mac animates windows.
 *
 * <p>On the shell's thread only.
 */
final class StartingWindows {
    private static final String TAG = "AimWindowShell";
    /** WMShell's starting_surface_brand_image_width and _height, in dp. */
    private static final int BRANDING_WIDTH_DP = 200;
    private static final int BRANDING_HEIGHT_DP = 80;
    /** WMShell's splash_icon_no_background_scale_factor. */
    private static final float NO_BACKGROUND_SCALE = 1.2f;
    /** com.android.internal.R.dimen.starting_surface_icon_size. */
    private static final int ICON_SIZE = com.android.internal.R.dimen.starting_surface_icon_size;
    /** The theme attributes read, in ascending order (obtainStyledAttributes). */
    private static final int[] ATTRS = {
        android.R.attr.colorBackground,
        android.R.attr.windowBackground,
        android.R.attr.windowSplashScreenBackground,
        android.R.attr.windowSplashScreenAnimatedIcon,
        android.R.attr.windowSplashScreenBrandingImage,
        android.R.attr.windowSplashScreenIconBackgroundColor,
    };
    private static final int COLOR_BACKGROUND = 0;
    private static final int WINDOW_BACKGROUND = 1;
    private static final int SPLASH_BACKGROUND = 2;
    private static final int SPLASH_ICON = 3;
    private static final int SPLASH_BRANDING = 4;
    private static final int SPLASH_ICON_BACKGROUND = 5;

    private final Context mContext;
    private final Handler mHandler;
    /** The splash window of each task that has one. */
    private final Map<Integer, Splash> mSplashes = new HashMap<>();
    /** The splash screens handed to their apps, by task. */
    private final Map<Integer, SplashScreenView> mCopied = new HashMap<>();

    private static final class Splash {
        final FrameLayout root;
        final SplashScreenView view;

        Splash(FrameLayout root, SplashScreenView view) {
            this.root = root;
            this.view = view;
        }
    }

    StartingWindows(Context context, Handler handler) {
        mContext = context;
        mHandler = handler;
    }

    /** WindowManager wants a starting window for `info`'s activity. */
    void add(StartingWindowInfo info) {
        int type = type(info);
        if (type == StartingWindowInfo.STARTING_WINDOW_TYPE_NONE) {
            return;
        }
        RunningTaskInfo task = info.taskInfo;
        ActivityInfo activity = info.targetActivityInfo != null
                ? info.targetActivityInfo : task.topActivityInfo;
        if (activity == null || activity.packageName == null) {
            return;
        }
        Display display = mContext.getSystemService(DisplayManager.class)
                .getDisplay(task.displayId);
        Context context = context(info, activity);
        if (display == null || context == null) {
            return;
        }
        try {
            show(info, activity, display, context, type);
        } catch (RuntimeException e) {
            // The app's resources failed to load (an app on storage that
            // went away): no starting window, as when the type is none.
            Slog.w(TAG, "task " + task.taskId + ": no starting window", e);
        }
    }

    private void show(StartingWindowInfo info, ActivityInfo activity, Display display,
            Context context, int type) {
        RunningTaskInfo task = info.taskInfo;
        boolean legacy = type == StartingWindowInfo.STARTING_WINDOW_TYPE_LEGACY_SPLASH_SCREEN;
        SplashScreenView view;
        TypedArray a = context.obtainStyledAttributes(ATTRS);
        try {
            view = view(context, activity, a, type, info.allowHandleSolidColorSplashScreen());
        } finally {
            a.recycle();
        }
        WindowManager.LayoutParams params = new WindowManager.LayoutParams(
                WindowManager.LayoutParams.TYPE_APPLICATION_STARTING);
        params.setFitInsetsSides(0);
        params.setFitInsetsTypes(0);
        params.format = legacy ? PixelFormat.OPAQUE : PixelFormat.TRANSLUCENT;
        params.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS;
        // Not touchable or focusable: the activity's own window takes input.
        params.flags = WindowManager.LayoutParams.FLAG_HARDWARE_ACCELERATED
                | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN
                | WindowManager.LayoutParams.FLAG_LAYOUT_INSET_DECOR
                | WindowManager.LayoutParams.FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS
                | WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE
                | WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                | WindowManager.LayoutParams.FLAG_ALT_FOCUSABLE_IM
                | (info.isKeyguardOccluded ? WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED : 0);
        params.token = info.appToken;
        params.packageName = activity.packageName;
        params.privateFlags |= WindowManager.LayoutParams.SYSTEM_FLAG_SHOW_FOR_ALL_USERS;
        params.setTitle("Splash Screen " + activity.packageName);
        // The window's own views take the system theme, not the app's.
        FrameLayout root = new FrameLayout(new ContextThemeWrapper(context, mContext.getTheme()));
        root.setPadding(0, 0, 0, 0);
        root.setFitsSystemWindows(false);
        root.addView(view);
        try {
            WindowManagerGlobal.getInstance().addView(root, params, display, null,
                    task.userId);
        } catch (WindowManager.BadTokenException e) {
            // The activity is gone or already has its window.
            Slog.w(TAG, "task " + task.taskId + ": no starting window: " + e.getMessage());
        }
        if (root.getParent() == null) {
            return;
        }
        remove(task.taskId);
        mSplashes.put(task.taskId, new Splash(root, view));
    }

    /** The task's content is ready: its starting window goes. */
    void remove(StartingWindowRemovalInfo info) {
        remove(info.taskId);
    }

    private void remove(int taskId) {
        Splash splash = mSplashes.remove(taskId);
        if (splash != null && splash.root.getParent() != null) {
            WindowManagerGlobal.getInstance().removeView(splash.root, false);
        }
    }

    /**
     * The app asked for the splash screen (an exit animation listener):
     * hands it a copy, or null when it cannot have one.
     */
    void copy(int taskId) {
        Splash splash = mSplashes.get(taskId);
        SplashScreenViewParcelable parcelable = null;
        if (splash != null && splash.view.isCopyable()) {
            SplashScreenView view = splash.view;
            parcelable = new SplashScreenViewParcelable(view);
            parcelable.setClientCallback(new RemoteCallback(result ->
                    mHandler.post(() -> appRemoved(taskId))));
            view.onCopied();
            mCopied.put(taskId, view);
        }
        ActivityTaskManager.getInstance().onSplashScreenViewCopyFinished(taskId, parcelable);
    }

    /** The app removed the splash screen it was handed. */
    void appRemoved(int taskId) {
        SplashScreenView view = mCopied.remove(taskId);
        if (view != null && view.getSurfaceHost() != null) {
            SplashScreenView.releaseIconHost(view.getSurfaceHost());
        }
    }

    /**
     * The starting window `info` gets: WMShell's choice for phones
     * (PhoneStartingWindowTypeAlgorithm), except that a task switch WMShell
     * shows a snapshot of gets none, and a windowless one (predictive back)
     * none.
     */
    private static int type(StartingWindowInfo info) {
        int p = info.startingWindowTypeParameter;
        boolean topIsHome = info.taskInfo.topActivityType == WindowConfiguration.ACTIVITY_TYPE_HOME;
        if ((p & StartingWindowInfo.TYPE_PARAMETER_WINDOWLESS) != 0) {
            return StartingWindowInfo.STARTING_WINDOW_TYPE_NONE;
        }
        boolean taskSwitch = (p & StartingWindowInfo.TYPE_PARAMETER_TASK_SWITCH) != 0;
        if (!topIsHome && ((p & StartingWindowInfo.TYPE_PARAMETER_PROCESS_RUNNING) == 0
                || (p & StartingWindowInfo.TYPE_PARAMETER_NEW_TASK) != 0
                || (taskSwitch && (p & StartingWindowInfo.TYPE_PARAMETER_ACTIVITY_CREATED) == 0))) {
            return splashType(p);
        }
        if (taskSwitch) {
            if ((p & StartingWindowInfo.TYPE_PARAMETER_ALLOW_TASK_SNAPSHOT) != 0) {
                return info.taskSnapshot != null || topIsHome
                        ? StartingWindowInfo.STARTING_WINDOW_TYPE_NONE
                        : StartingWindowInfo.STARTING_WINDOW_TYPE_SOLID_COLOR_SPLASH_SCREEN;
            }
            if ((p & StartingWindowInfo.TYPE_PARAMETER_ACTIVITY_DRAWN) == 0 && !topIsHome) {
                return splashType(p);
            }
        }
        return StartingWindowInfo.STARTING_WINDOW_TYPE_NONE;
    }

    private static int splashType(int p) {
        if ((p & StartingWindowInfo.TYPE_PARAMETER_USE_SOLID_COLOR_SPLASH_SCREEN) != 0) {
            return StartingWindowInfo.STARTING_WINDOW_TYPE_SOLID_COLOR_SPLASH_SCREEN;
        }
        return (p & StartingWindowInfo.TYPE_PARAMETER_LEGACY_SPLASH_SCREEN) != 0
                ? StartingWindowInfo.STARTING_WINDOW_TYPE_LEGACY_SPLASH_SCREEN
                : StartingWindowInfo.STARTING_WINDOW_TYPE_SPLASH_SCREEN;
    }

    /**
     * The app's context with its splash screen theme: the one its launch
     * asked for, else the activity's, else the device default; in the
     * task's night mode.
     */
    private Context context(StartingWindowInfo info, ActivityInfo activity) {
        int theme = info.splashScreenThemeResId != 0 ? info.splashScreenThemeResId
                : activity.getThemeResource() != 0 ? activity.getThemeResource()
                : android.R.style.Theme_DeviceDefault_DayNight;
        Context context;
        try {
            context = mContext.createPackageContextAsUser(activity.packageName,
                    Context.CONTEXT_RESTRICTED, UserHandle.of(info.taskInfo.userId));
        } catch (PackageManager.NameNotFoundException e) {
            Slog.w(TAG, "no package " + activity.packageName + " for its starting window");
            return null;
        }
        context.setTheme(theme);
        Configuration taskConfig = info.taskInfo.getConfiguration();
        if ((taskConfig.uiMode & Configuration.UI_MODE_NIGHT_MASK)
                != (context.getResources().getConfiguration().uiMode
                        & Configuration.UI_MODE_NIGHT_MASK)) {
            context = context.createConfigurationContext(taskConfig);
            context.setTheme(theme);
        }
        return context;
    }

    /**
     * The splash screen of `type`: the theme's splash screen background
     * (else its window background's color), its splash icon (else the
     * activity's icon) and branding image; a legacy one shows the window
     * background.
     */
    private static SplashScreenView view(Context context, ActivityInfo activity, TypedArray a,
            int type, boolean allowSolidColor) {
        float density = context.getResources().getDisplayMetrics().density;
        int background = backgroundColor(a);
        SplashScreenView.Builder builder = new SplashScreenView.Builder(context)
                .setBackgroundColor(background)
                .setAllowHandleSolidColor(allowSolidColor);
        if (type == StartingWindowInfo.STARTING_WINDOW_TYPE_LEGACY_SPLASH_SCREEN) {
            return builder.setOverlayDrawable(a.getDrawable(WINDOW_BACKGROUND)).build();
        }
        if (type == StartingWindowInfo.STARTING_WINDOW_TYPE_SPLASH_SCREEN) {
            int iconSize = context.getResources().getDimensionPixelSize(ICON_SIZE);
            int iconBackground = a.getColor(SPLASH_ICON_BACKGROUND, Color.TRANSPARENT);
            boolean drawBackground =
                    iconBackground != Color.TRANSPARENT && iconBackground != background;
            Drawable icon = a.getDrawable(SPLASH_ICON);
            if (icon == null) {
                icon = activity.loadIcon(context.getPackageManager());
            } else if (!drawBackground) {
                // Nothing below the theme's icon: scaled up, as WMShell does.
                iconSize = Math.round(iconSize * NO_BACKGROUND_SCALE);
            }
            if (icon instanceof Animatable) {
                icon = new SplashIcons.AnimatedIcon(icon);
            } else if (icon instanceof AdaptiveIconDrawable) {
                // It has a background of its own.
                drawBackground = false;
            } else {
                icon = new SplashIcons.MaskedForeground(icon);
            }
            if (drawBackground) {
                builder.setIconBackground(new SplashIcons.MaskBackground(iconBackground));
            }
            builder.setIconSize(iconSize)
                    .setCenterViewDrawable(icon)
                    .setBrandingDrawable(a.getDrawable(SPLASH_BRANDING),
                            Math.round(BRANDING_WIDTH_DP * density),
                            Math.round(BRANDING_HEIGHT_DP * density));
        }
        return builder.build();
    }

    private static int backgroundColor(TypedArray a) {
        int color = a.getColor(SPLASH_BACKGROUND, Color.TRANSPARENT);
        if (color != Color.TRANSPARENT) {
            return color;
        }
        Drawable window = a.getDrawable(WINDOW_BACKGROUND);
        if (window instanceof ColorDrawable) {
            return ((ColorDrawable) window).getColor();
        }
        return a.getColor(COLOR_BACKGROUND, Color.TRANSPARENT);
    }
}
