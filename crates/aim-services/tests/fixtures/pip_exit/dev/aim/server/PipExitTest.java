package dev.aim.server;

import android.graphics.Rect;
import android.window.WindowContainerToken;
import android.window.WindowContainerTransaction;

/** Pinned PipTaskOrganizer's exit contract, with desktop's explicit freeform mode. */
public final class PipExitTest {
    public static void main(String[] args) {
        Rect entry = new Rect(120, 80, 920, 680);
        WindowShell.Pip pip = new WindowShell.Pip(entry, 16f / 9);
        // WM may reuse its Rect and the app may change PiP's aspect ratio.
        entry.right = 300;
        pip.ratio = 1;
        WindowContainerToken token = new WindowContainerToken();
        for (boolean desktop : new boolean[] {true, false}) {
            WindowContainerTransaction wct = pip.exit(token, desktop);
            if (wct.token != token || wct.mode != (desktop ? 5 : 0) || wct.activityMode != 0) {
                throw new AssertionError("exit modes");
            }
            Rect b = wct.bounds;
            if (b.left != 120 || b.top != 80 || b.right != 920 || b.bottom != 680) {
                throw new AssertionError("entry bounds must survive PiP resize");
            }
        }
    }
}
