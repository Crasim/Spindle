package org.spindleapp.spindle;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Context;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.util.List;

/** Read-only handoff of a specific app-private update to Android's package installer. */
public final class UpdateApkProvider extends ContentProvider {
    static final String MIME = "application/vnd.android.package-archive";
    private static final String APK = "Spindle.apk";

    static Uri uri(Context context, String version) {
        return new Uri.Builder().scheme("content").authority(context.getPackageName() + ".updates")
                .appendPath("release").appendPath(version).appendPath(APK).build();
    }

    static File file(Context context, String version) throws IOException {
        if (!version.matches("[0-9]+[.][0-9]+[.][0-9]+(?:-[A-Za-z0-9-]+(?:[.][A-Za-z0-9-]+)*)?")) {
            throw new FileNotFoundException("Invalid update version");
        }
        File root = new File(context.getFilesDir(), "data/spindle/updates").getCanonicalFile();
        File apk = new File(new File(root, version), APK).getCanonicalFile();
        if (!apk.getPath().startsWith(root.getPath() + File.separator) || !apk.isFile()) {
            throw new FileNotFoundException("The update APK is unavailable");
        }
        return apk;
    }

    private File file(Uri uri) throws IOException {
        List<String> parts = uri.getPathSegments();
        if (parts.size() != 3 || !parts.get(0).equals("release") || !parts.get(2).equals(APK)) {
            throw new FileNotFoundException("Unknown update URI");
        }
        return file(getContext(), parts.get(1));
    }

    @Override public boolean onCreate() { return true; }
    @Override public String getType(Uri uri) { return MIME; }

    @Override public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) throw new FileNotFoundException("Read-only update");
        try {
            return ParcelFileDescriptor.open(file(uri), ParcelFileDescriptor.MODE_READ_ONLY);
        } catch (IOException error) {
            throw new FileNotFoundException("The update APK is unavailable");
        }
    }

    @Override public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sort) {
        String[] columns = projection == null
                ? new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE} : projection;
        MatrixCursor cursor = new MatrixCursor(columns);
        try {
            File apk = file(uri);
            Object[] row = new Object[columns.length];
            for (int i = 0; i < columns.length; i++) {
                if (OpenableColumns.DISPLAY_NAME.equals(columns[i])) row[i] = APK;
                if (OpenableColumns.SIZE.equals(columns[i])) row[i] = apk.length();
            }
            cursor.addRow(row);
        } catch (IOException ignored) { }
        return cursor;
    }

    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException(); }
    @Override public int update(Uri uri, ContentValues values, String selection, String[] args) { throw new UnsupportedOperationException(); }
    @Override public int delete(Uri uri, String selection, String[] args) { throw new UnsupportedOperationException(); }
}
