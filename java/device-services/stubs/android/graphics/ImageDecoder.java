// Compile-only Android 16 API; checked against the original image at build time.
package android.graphics;

public final class ImageDecoder {
    private ImageDecoder(long nativePtr, int width, int height, boolean animated, boolean isNinePatch) {}
    public static final int ALLOCATOR_SOFTWARE = 1;
    public static class Source {}
    public static class ImageInfo {
        private ImageInfo(android.util.Size size, boolean isAnimated, String mimeType, ColorSpace colorSpace) {}
        public android.util.Size getSize() { throw new RuntimeException("stub"); }
    }
    public interface OnHeaderDecodedListener {
        void onHeaderDecoded(ImageDecoder decoder, ImageInfo info, Source source);
    }
    public static Source createSource(android.content.ContentResolver resolver, android.net.Uri uri) {
        throw new RuntimeException("stub");
    }
    public static Bitmap decodeBitmap(Source source, OnHeaderDecodedListener listener)
            throws java.io.IOException { throw new RuntimeException("stub"); }
    public void setAllocator(int allocator) { throw new RuntimeException("stub"); }
    public void setTargetSize(int width, int height) { throw new RuntimeException("stub"); }
}
