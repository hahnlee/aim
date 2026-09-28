package dev.aim.probe;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileNotFoundException;
import java.net.URLConnection;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/** Runtime-owned document boundary between macOS files and an unchanged APK. */
public final class ProbeHostDocumentProvider extends ContentProvider {
    public static final String AUTHORITY = "dev.aim.hostfiles";
    private static final AtomicLong NEXT_ID = new AtomicLong(1L);
    private static final ConcurrentHashMap<String, Document> DOCUMENTS =
            new ConcurrentHashMap<>();

    private static final class Document {
        final File staging;
        final File destination;

        Document(File staging, File destination) {
            this.staging = staging;
            this.destination = destination;
        }

        boolean writable() {
            return destination != null;
        }
    }

    public static Uri registerImport(String stagedPath) throws FileNotFoundException {
        File file = new File(stagedPath);
        if (!file.isFile()) throw new FileNotFoundException(stagedPath);
        String id = Long.toHexString(NEXT_ID.getAndIncrement());
        DOCUMENTS.put(id, new Document(file, null));
        return new Uri.Builder().scheme("content")
                .authority(AUTHORITY).appendPath("import").appendPath(id).build();
    }

    public static Uri registerExport(String stagedPath, String destinationPath)
            throws FileNotFoundException {
        File staging = new File(stagedPath);
        File destination = new File(destinationPath);
        File parent = destination.getParentFile();
        if (parent == null || (!parent.isDirectory() && !parent.mkdirs())) {
            throw new FileNotFoundException(destinationPath);
        }
        String id = Long.toHexString(NEXT_ID.getAndIncrement());
        DOCUMENTS.put(id, new Document(staging, destination));
        return new Uri.Builder().scheme("content")
                .authority(AUTHORITY).appendPath("export").appendPath(id).build();
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public String getType(Uri uri) {
        Document document = resolve(uri);
        if (document == null) return null;
        File file = document.writable() ? document.destination : document.staging;
        String name = file.getName().toLowerCase();
        String guessed = URLConnection.guessContentTypeFromName(name);
        return guessed == null ? "application/octet-stream" : guessed;
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode)
            throws FileNotFoundException {
        Document document = resolve(uri);
        if (document == null) {
            throw new FileNotFoundException(String.valueOf(uri));
        }
        if (!document.writable()) {
            if (!"r".equals(mode)) throw new FileNotFoundException(String.valueOf(uri));
            android.util.Log.i("DarwinHostDocument", "open read uri=" + uri
                    + " path=" + document.staging);
            return ParcelFileDescriptor.open(
                    document.staging, ParcelFileDescriptor.MODE_READ_ONLY);
        }
        if (!mode.contains("w")) throw new FileNotFoundException(String.valueOf(uri));
        int flags = ParcelFileDescriptor.MODE_CREATE
                | ParcelFileDescriptor.MODE_TRUNCATE
                | ParcelFileDescriptor.MODE_WRITE_ONLY;
        android.util.Log.i("DarwinHostDocument", "open write uri=" + uri
                + " mode=" + mode + " destination=" + document.destination);
        // NSSavePanel grants this process the destination. Expose that file
        // directly through a normal PFD: Android providers are allowed to do
        // so, it avoids an extra staging copy, and it does not require the
        // reliable-PFD communication socket used only for close callbacks.
        return ParcelFileDescriptor.open(document.destination, flags);
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection,
            String[] selectionArgs, String sortOrder) {
        Document document = resolve(uri);
        File file = document == null ? null
                : (document.writable() ? document.destination : document.staging);
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
    public Uri insert(Uri uri, ContentValues values) {
        throw new UnsupportedOperationException("host imports are read-only");
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection,
            String[] selectionArgs) {
        return 0;
    }

    private static Document resolve(Uri uri) {
        if (uri == null || !AUTHORITY.equals(uri.getAuthority())
                || uri.getPathSegments().size() != 2
                || (!("import".equals(uri.getPathSegments().get(0)))
                    && !("export".equals(uri.getPathSegments().get(0))))) {
            return null;
        }
        return DOCUMENTS.get(uri.getPathSegments().get(1));
    }

}
