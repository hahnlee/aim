package dev.aim.server;

/** The native frame prefix of an original Transformation's Matrix.getValues(). */
final class ActivityAnimationSample {
    static void write(float[] samples, int at, float[] matrix, float alpha) {
        // SurfaceControl.Transaction.setMatrix: dsdx, dtdx, dtdy, dsdy.
        samples[at] = matrix[0]; samples[at + 1] = matrix[3];
        samples[at + 2] = matrix[1]; samples[at + 3] = matrix[4];
        samples[at + 4] = matrix[2]; samples[at + 5] = matrix[5];
        samples[at + 6] = alpha;
    }
}
