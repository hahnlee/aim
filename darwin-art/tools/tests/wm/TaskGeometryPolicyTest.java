package dev.darwinart.runtime.wm;

public final class TaskGeometryPolicyTest {
    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void extent(int[] actual, int width, int height) {
        check(actual[0] == width && actual[1] == height,
                "extent " + actual[0] + "x" + actual[1] + " != " + width + "x" + height);
    }

    public static void main(String[] args) {
        // Fixed orientations choose a shape; user/sensor/unspecified keep the window.
        for (int landscape : new int[] {0, 6, 8, 11}) {
            check(TaskGeometryPolicy.requiredShape(landscape) == TaskGeometryPolicy.LANDSCAPE,
                    "landscape " + landscape);
        }
        for (int portrait : new int[] {1, 7, 9, 12}) {
            check(TaskGeometryPolicy.requiredShape(portrait) == TaskGeometryPolicy.PORTRAIT,
                    "portrait " + portrait);
        }
        for (int free : new int[] {-1, 2, 3, 4, 5, 10, 13, 14}) {
            check(TaskGeometryPolicy.requiredShape(free) == TaskGeometryPolicy.NONE,
                    "free " + free);
        }
        extent(TaskGeometryPolicy.extentFor(720, 1280, TaskGeometryPolicy.LANDSCAPE), 1280, 720);
        extent(TaskGeometryPolicy.extentFor(1280, 720, TaskGeometryPolicy.LANDSCAPE), 1280, 720);
        extent(TaskGeometryPolicy.extentFor(1280, 720, TaskGeometryPolicy.PORTRAIT), 720, 1280);
        extent(TaskGeometryPolicy.extentFor(900, 700, TaskGeometryPolicy.NONE), 900, 700);

        // Unity-style activity handles every size/orientation change: no relaunch.
        int unity = 0x40003fff | 0x3;
        int resize = 0x0400 | 0x0800 | 0x0080 | 0x0100; // size, smallest, orientation, layout
        check(!TaskGeometryPolicy.shouldRelaunch(resize,
                TaskGeometryPolicy.handledChanges(unity, 34)), "unity relaunch");
        // An activity declaring nothing is relaunched by the framework path.
        check(TaskGeometryPolicy.shouldRelaunch(resize,
                TaskGeometryPolicy.handledChanges(0x3, 34)), "plain relaunch");
        // Pre-HONEYCOMB_MR2 apps implicitly handle screen size, not orientation.
        int legacy = TaskGeometryPolicy.handledChanges(0x0080, 10);
        check(!TaskGeometryPolicy.shouldRelaunch(0x0400 | 0x0800 | 0x0080, legacy), "legacy");
        // Window-configuration-only changes never relaunch.
        check(TaskGeometryPolicy.reportableChanges(0x20000000 | 0x80000000) == 0, "window only");

        check(TaskGeometryPolicy.pixelsFromPoints(640, 2) == 1280, "px");
        check(TaskGeometryPolicy.pointsFromPixels(1281, 2) == 641, "points round up");
        System.out.println("TaskGeometryPolicy PASS");
    }
}
