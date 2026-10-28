// Compile-only Android 16 API; checked against the original image at build time.
package android.graphics;

public abstract class ColorSpace {
    ColorSpace(String name, Model model, int id) {}
    public enum Model {
        ;
        Model(int componentCount) {}
    }
}
