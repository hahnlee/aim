package dev.aim.probe;

import android.graphics.Canvas;
import android.graphics.Paint;

/** Software Canvas target owned by the Darwin host-window policy. */
public final class ProbeCanvas extends Canvas {
    private int width;
    private int height;
    private int[] pixels;
    private int saveCount;

    public ProbeCanvas() {
        // The host launcher allocates this object without invoking a
        // constructor. Calling Canvas() would require the Android Skia JNI
        // backend before that backend has been ported to Darwin.
    }

    private void initialize(int newWidth, int newHeight) {
        width = newWidth;
        height = newHeight;
        pixels = new int[width * height];
        saveCount = 1;
    }

    private int[] snapshot() {
        return pixels;
    }

    @Override
    public int getWidth() {
        return width;
    }

    @Override
    public int getHeight() {
        return height;
    }

    @Override
    public boolean isHardwareAccelerated() {
        return false;
    }

    @Override
    public int save() {
        return saveCount++;
    }

    @Override
    public int getSaveCount() {
        return saveCount;
    }

    @Override
    public void restore() {
        if (saveCount > 1) {
            saveCount--;
        }
    }

    @Override
    public void restoreToCount(int count) {
        saveCount = Math.max(1, count);
    }

    @Override
    public boolean quickReject(float left, float top, float right, float bottom) {
        return false;
    }

    @Override
    public void translate(float dx, float dy) {
        // The first hierarchy is positioned at the origin. A transform stack
        // is added when nested/offset content becomes part of the gate.
    }

    @Override
    public boolean clipRect(int left, int top, int right, int bottom) {
        return left < right && top < bottom;
    }

    @Override
    public boolean clipRect(float left, float top, float right, float bottom) {
        return left < right && top < bottom;
    }

    @Override
    public void drawColor(int color) {
        for (int index = 0; index < pixels.length; index++) {
            pixels[index] = color;
        }
    }

    @Override
    public void drawBitmap(
            int[] colors,
            int offset,
            int stride,
            int x,
            int y,
            int bitmapWidth,
            int bitmapHeight,
            boolean hasAlpha,
            Paint paint) {
        copyBitmap(colors, offset, stride, x, y, bitmapWidth, bitmapHeight);
    }

    @Override
    public void drawBitmap(
            int[] colors,
            int offset,
            int stride,
            float x,
            float y,
            int bitmapWidth,
            int bitmapHeight,
            boolean hasAlpha,
            Paint paint) {
        copyBitmap(colors, offset, stride, (int) x, (int) y, bitmapWidth, bitmapHeight);
    }

    private void copyBitmap(
            int[] colors,
            int offset,
            int stride,
            int x,
            int y,
            int bitmapWidth,
            int bitmapHeight) {
        for (int row = 0; row < bitmapHeight; row++) {
            int destinationY = y + row;
            if (destinationY < 0 || destinationY >= height) {
                continue;
            }
            for (int column = 0; column < bitmapWidth; column++) {
                int destinationX = x + column;
                if (destinationX >= 0 && destinationX < width) {
                    pixels[destinationY * width + destinationX] =
                            colors[offset + row * stride + column];
                }
            }
        }
    }
}
