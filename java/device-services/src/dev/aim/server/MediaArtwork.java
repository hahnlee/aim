package dev.aim.server;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.ImageDecoder;
import android.net.Uri;
import android.os.Binder;
import android.os.ServiceManager;
import android.util.Slog;

import java.io.ByteArrayOutputStream;
import java.io.IOException;

/** MediaDataLoader's URI schemes and software decoder, android-16.0.0_r1. */
public final class MediaArtwork extends IMediaArtwork.Stub {
    private final Context context;

    private MediaArtwork(Context context) { this.context = context; }

    public static void publish(Context context) {
        ServiceManager.addService("aim.media_artwork", new MediaArtwork(context));
    }

    @Override
    public byte[] load(String value) {
        Bridge.enforceSystemUid();
        Uri uri = Uri.parse(value);
        String scheme = uri.getScheme();
        if (!"content".equals(scheme) && !"file".equals(scheme)
                && !"android.resource".equals(scheme)) {
            Slog.w("AimMediaArtwork", "Invalid album art URI: " + uri);
            return null;
        }
        long identity = Binder.clearCallingIdentity();
        try {
            Bitmap bitmap = ImageDecoder.decodeBitmap(
                    ImageDecoder.createSource(context.getContentResolver(), uri),
                    (decoder, info, source) -> {
                        decoder.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE);
                        int width = info.getSize().getWidth();
                        int height = info.getSize().getHeight();
                        double scale = Math.min(1, 256.0 / Math.max(width, height));
                        decoder.setTargetSize(Math.max(1, (int) (width * scale)),
                                Math.max(1, (int) (height * scale)));
                    });
            try {
                ByteArrayOutputStream bytes = new ByteArrayOutputStream();
                if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, bytes)) {
                    throw new IOException("PNG compression failed");
                }
                return bytes.toByteArray();
            } finally {
                bitmap.recycle();
            }
        } catch (IOException | SecurityException | IllegalArgumentException e) {
            Slog.w("AimMediaArtwork", "Cannot load album art: " + uri, e);
            return null;
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }
}
