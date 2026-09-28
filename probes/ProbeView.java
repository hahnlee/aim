package dev.aim.probe;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.view.View;

/** Activity content traversed through the real PhoneWindow and DecorView. */
public final class ProbeView extends View {
    private static final int WIDTH = 640;
    private static final int HEIGHT = 360;

    private boolean presented;

    public ProbeView(Context context) {
        super(context);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        if (canvas.getClass() != Canvas.class) {
            drawProbeBackend(canvas);
            presented = true;
            return;
        }
        Paint paint = new Paint();
        canvas.drawColor(0xff111827);
        drawRect(canvas, paint, 28, 28, WIDTH - 56, HEIGHT - 56, 0xfff8fafc);
        drawRect(canvas, paint, 28, 28, WIDTH - 56, 70, 0xff3ddc84);
        drawRect(canvas, paint, 76, 144, WIDTH - 152, 112, 0xffe2e8f0);
        drawRect(canvas, paint, 188, 278, WIDTH - 376, 48, 0xff2563eb);
        drawArtMark(canvas, paint, 76, 52, 0xff102a20);
        presented = true;
    }

    public boolean wasPresented() {
        return presented;
    }

    private void drawRect(
            Canvas canvas, Paint paint, int left, int top, int width, int height, int color) {
        paint.setColor(color);
        canvas.drawRect(left, top, left + width, top + height, paint);
    }

    private void drawArtMark(Canvas canvas, Paint paint, int left, int top, int color) {
        drawRect(canvas, paint, left, top + 24, 12, 30, color);
        drawRect(canvas, paint, left + 12, top + 12, 12, 12, color);
        drawRect(canvas, paint, left + 24, top + 24, 12, 30, color);
        drawRect(canvas, paint, left + 7, top + 33, 24, 9, color);

        drawRect(canvas, paint, left + 52, top + 12, 12, 42, color);
        drawRect(canvas, paint, left + 64, top + 12, 24, 9, color);
        drawRect(canvas, paint, left + 76, top + 21, 12, 15, color);
        drawRect(canvas, paint, left + 64, top + 33, 18, 9, color);
        drawRect(canvas, paint, left + 76, top + 42, 12, 12, color);

        drawRect(canvas, paint, left + 104, top + 12, 48, 9, color);
        drawRect(canvas, paint, left + 122, top + 21, 12, 33, color);
    }

    private static void drawProbeBackend(Canvas canvas) {
        int[] pixels = new int[WIDTH * HEIGHT];
        fill(pixels, 0, 0, WIDTH, HEIGHT, 0xff111827);
        fill(pixels, 28, 28, WIDTH - 56, HEIGHT - 56, 0xfff8fafc);
        fill(pixels, 28, 28, WIDTH - 56, 70, 0xff3ddc84);
        fill(pixels, 76, 144, WIDTH - 152, 112, 0xffe2e8f0);
        fill(pixels, 188, 278, WIDTH - 376, 48, 0xff2563eb);
        drawProbeArtMark(pixels, 76, 52, 0xff102a20);
        canvas.drawBitmap(pixels, 0, WIDTH, 0, 0, WIDTH, HEIGHT, true, null);
    }

    private static void fill(
            int[] pixels, int left, int top, int width, int height, int color) {
        for (int y = top; y < top + height; y++) {
            int row = y * WIDTH;
            for (int x = left; x < left + width; x++) {
                pixels[row + x] = color;
            }
        }
    }

    private static void drawProbeArtMark(int[] pixels, int left, int top, int color) {
        fill(pixels, left, top + 24, 12, 30, color);
        fill(pixels, left + 12, top + 12, 12, 12, color);
        fill(pixels, left + 24, top + 24, 12, 30, color);
        fill(pixels, left + 7, top + 33, 24, 9, color);
        fill(pixels, left + 52, top + 12, 12, 42, color);
        fill(pixels, left + 64, top + 12, 24, 9, color);
        fill(pixels, left + 76, top + 21, 12, 15, color);
        fill(pixels, left + 64, top + 33, 18, 9, color);
        fill(pixels, left + 76, top + 42, 12, 12, color);
        fill(pixels, left + 104, top + 12, 48, 9, color);
        fill(pixels, left + 122, top + 21, 12, 33, color);
    }
}
