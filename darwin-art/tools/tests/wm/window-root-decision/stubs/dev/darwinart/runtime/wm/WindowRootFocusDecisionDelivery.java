package dev.darwinart.runtime.wm;

import java.util.ArrayDeque;

final class WindowRootFocusDecisionDelivery {
    private static final ArrayDeque<Runnable> QUEUE = new ArrayDeque<>();
    private static final ArrayDeque<Integer> DELAYS = new ArrayDeque<>();

    WindowRootFocusDecisionDelivery() {}

    void schedule(Runnable action, int delay) {
        QUEUE.addLast(action);
        DELAYS.addLast(Integer.valueOf(delay));
    }

    static int size() { return QUEUE.size(); }
    static int nextDelay() { return DELAYS.isEmpty() ? -1 : DELAYS.peekFirst().intValue(); }
    static void runNext() {
        if (QUEUE.isEmpty()) throw new AssertionError("scheduler is empty");
        DELAYS.removeFirst();
        QUEUE.removeFirst().run();
    }
    static void clear() {
        QUEUE.clear();
        DELAYS.clear();
    }
}
