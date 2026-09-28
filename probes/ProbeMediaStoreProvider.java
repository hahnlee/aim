package dev.aim.probe;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.MediaStore;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileNotFoundException;
import java.net.URLConnection;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/** App-scoped MediaStore boundary used when Android publishes downloads. */
final class ProbeMediaStoreProvider extends ContentProvider {
    static final String AUTHORITY = "media";
    private static final AtomicLong NEXT_ID = new AtomicLong(1L);
    private static final ConcurrentHashMap<String, File> FILES =
            new ConcurrentHashMap<>();

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public Uri insert(Uri collection, ContentValues values) {
        String name = values.getAsString(MediaStore.MediaColumns.DISPLAY_NAME);
        String relative = values.getAsString(MediaStore.MediaColumns.RELATIVE_PATH);
        String guestData = System.getenv("AIM_APK_APP_DATA_GUEST_DIR");
        if (name == null || name.isEmpty() || name.indexOf('/') >= 0
                || name.indexOf('\\') >= 0 || guestData == null) {
            return null;
        }
        File root = new File(guestData, "external/files");
        File directory = relative == null || relative.isEmpty()
                ? root : new File(root, relative);
        try {
            String canonicalRoot = root.getCanonicalPath() + File.separator;
            String canonicalDirectory = directory.getCanonicalPath() + File.separator;
            if (!canonicalDirectory.startsWith(canonicalRoot)
                    || (!directory.isDirectory() && !directory.mkdirs())) {
                return null;
            }
            File destination = new File(directory, name);
            String id = Long.toString(NEXT_ID.getAndIncrement());
            FILES.put(id, destination);
            return new Uri.Builder().scheme("content").authority(AUTHORITY)
                    .appendPath("external_primary").appendPath("file")
                    .appendPath(id).build();
        } catch (java.io.IOException error) {
            return null;
        }
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode)
            throws FileNotFoundException {
        File file = resolve(uri);
        if (file == null) throw new FileNotFoundException(String.valueOf(uri));
        int flags = mode != null && mode.contains("w")
                ? ParcelFileDescriptor.MODE_CREATE
                        | ParcelFileDescriptor.MODE_TRUNCATE
                        | ParcelFileDescriptor.MODE_WRITE_ONLY
                : ParcelFileDescriptor.MODE_READ_ONLY;
        android.util.Log.i("DarwinMediaStore", "open uri=" + uri
                + " mode=" + mode + " path=" + file);
        return ParcelFileDescriptor.open(file, flags);
    }

    @Override
    public String getType(Uri uri) {
        File file = resolve(uri);
        String guessed = file == null ? null
                : URLConnection.guessContentTypeFromName(file.getName());
        return guessed == null ? "application/octet-stream" : guessed;
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection,
            String[] selectionArgs, String sortOrder) {
        File file = resolve(uri);
        String[] columns = projection == null
                ? new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE}
                : projection;
        MatrixCursor cursor = new MatrixCursor(columns);
        if (file == null) return cursor;
        Object[] row = new Object[columns.length];
        for (int index = 0; index < columns.length; index++) {
            if (OpenableColumns.DISPLAY_NAME.equals(columns[index])) {
                row[index] = file.getName();
            } else if (OpenableColumns.SIZE.equals(columns[index])) {
                row[index] = Long.valueOf(file.length());
            }
        }
        cursor.addRow(row);
        return cursor;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection,
            String[] selectionArgs) {
        return resolve(uri) == null ? 0 : 1;
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        File file = resolve(uri);
        return file != null && (!file.exists() || file.delete()) ? 1 : 0;
    }

    private static File resolve(Uri uri) {
        if (uri == null || !AUTHORITY.equals(uri.getAuthority())) return null;
        java.util.List<String> segments = uri.getPathSegments();
        if (segments.size() < 3) return null;
        return FILES.get(segments.get(segments.size() - 1));
    }
}
