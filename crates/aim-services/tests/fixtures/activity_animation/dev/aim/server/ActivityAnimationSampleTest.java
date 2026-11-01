package dev.aim.server;

import java.util.Arrays;

/** Matrix.getValues layout and SurfaceControl.Transaction.setMatrix, android-16.0.0_r1. */
public final class ActivityAnimationSampleTest {
    public static void main(String[] args) {
        float[][] matrices = {
            {1, 0, 0, 0, 1, 0, 0, 0, 1}, // identity
            {1, 0, 13, 0, 1, -17, 0, 0, 1}, // translation, in Android pixels
            {2, 0, 0, 0, 3, 0, 0, 0, 1}, // nonuniform scale
            {0, -1, 0, 1, 0, 0, 0, 0, 1}, // 90 degree rotation
            {2, 5, 13, 7, 3, -17, 0, 0, 1}, // distinguish every affine slot
        };
        float[][] nativeSlots = {{1, 0, 0, 1}, {1, 0, 0, 1}, {2, 0, 0, 3},
                {0, 1, -1, 0}, {2, 7, 5, 3}};
        for (int i = 0; i < matrices.length; i++) {
            float[] m = matrices[i];
            float[] samples = new float[33];
            Arrays.fill(samples, -99);
            ActivityAnimationSample.write(samples, 11, m, 0.25f);
            float[] expected = {nativeSlots[i][0], nativeSlots[i][1], nativeSlots[i][2],
                    nativeSlots[i][3], m[2], m[5], 0.25f};
            if (!Arrays.equals(Arrays.copyOfRange(samples, 11, 18), expected)) {
                throw new AssertionError("native slots " + i + ": " + Arrays.toString(samples));
            }
            for (int j = 0; j < samples.length; j++) {
                if ((j < 11 || j >= 18) && samples[j] != -99) {
                    throw new AssertionError("overwrote crop or adjacent frame");
                }
            }
            for (float v : m) System.out.print(v + " ");
            for (float v : Arrays.copyOfRange(samples, 11, 18)) System.out.print(v + " ");
            System.out.println();
        }
    }
}
