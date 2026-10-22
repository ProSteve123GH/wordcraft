package com.storytold.wordcraft;

import android.content.Intent;
import android.database.Cursor;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.util.Log;
import android.view.View;

import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;

import com.google.androidgamesdk.GameActivity;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Locale;

/**
 * WordCraft's Android activity.
 *
 * <p>GameActivity owns the SurfaceView, input dispatch, IME plumbing and the native lifecycle
 * callbacks. Rust receives the activity through {@code android_main}.</p>
 *
 * <p>The document canvas is the whole tablet screen. WordCraft draws its own ribbon and page; the
 * system bars stay hidden until the user swipes them in, and the soft keyboard resizes the game
 * surface through {@code adjustResize}.</p>
 *
 * <p>Documents are opened and saved through Android's Storage Access Framework. Rust calls the
 * {@code startOpenDocument} / {@code startSaveDocument} instance methods on the Java main thread,
 * the picker answers here, and bytes or save results go back to the native side through the static
 * {@code nativeOpenResult} / {@code nativeSaveResult} callbacks. Reading and writing happen on
 * worker threads so the UI thread never touches a document's contents.</p>
 */
public final class MainActivity extends GameActivity {
    private static final String TAG = "WordCraft";

    private static final int REQUEST_OPEN = 4101;
    private static final int REQUEST_SAVE = 4102;

    /** Largest document, picture or font file read in one pick (128 MiB). */
    private static final long MAX_OPEN_BYTES = 128L * 1024L * 1024L;
    /** Most files read from one multiple-file pick (fonts). */
    private static final int MAX_MULTI_FILES = 20;
    /** Longest display name handed to the engine. */
    private static final int MAX_NAME_CHARS = 255;

    /**
     * The document last opened or saved. Save writes here without asking again; Save As and
     * exports always use the picker. Only touched on the Java main thread and the short-lived
     * write thread, hence {@code volatile}.
     */
    private volatile String currentDocumentUri;
    private volatile String currentDocumentName;

    /** The purpose of the open picker in flight, so a document pick updates the current file. */
    private String openPurpose = "document";

    /** A save waiting for the create-document picker; null when no picker is open. */
    private PendingSave pendingSave;

    private static final class PendingSave {
        final byte[] data;
        final String name;
        final boolean setCurrent;
        final long ticket;

        PendingSave(byte[] data, String name, boolean setCurrent, long ticket) {
            this.data = data;
            this.name = name;
            this.setCurrent = setCurrent;
            this.ticket = ticket;
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Log.i(TAG, "onCreate " + this + " task=" + getTaskId());
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        hideSystemBars();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideSystemBars();
        }
    }

    @SuppressWarnings("deprecation")
    private void hideSystemBars() {
        View decor = getWindow().getDecorView();
        WindowInsetsControllerCompat controller =
                WindowCompat.getInsetsController(getWindow(), decor);
        controller.hide(WindowInsetsCompat.Type.systemBars());
        controller.setSystemBarsBehavior(
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
        getWindow().setStatusBarColor(Color.TRANSPARENT);
        getWindow().setNavigationBarColor(Color.TRANSPARENT);
    }

    @Override
    protected void onStart() {
        super.onStart();
        Log.i(TAG, "onStart " + this);
    }

    @Override
    protected void onResume() {
        super.onResume();
        Log.i(TAG, "onResume " + this);
    }

    @Override
    protected void onPause() {
        Log.i(TAG, "onPause " + this);
        super.onPause();
    }

    @Override
    protected void onStop() {
        Log.i(TAG, "onStop " + this);
        super.onStop();
    }

    @Override
    protected void onDestroy() {
        Log.i(TAG, "onDestroy entered " + this + " changing=" + isChangingConfigurations());
        super.onDestroy();
        Log.i(TAG, "onDestroy returned " + this);
    }

    // ------------------------------------------------------------------------------------
    // Called from Rust on the Java main thread.
    // ------------------------------------------------------------------------------------

    /**
     * Opens Android's document picker. {@code purpose} is {@code document}, {@code picture},
     * {@code font} or {@code recipients}; pictures are limited to images, everything else accepts
     * any file the provider exposes. {@code multiple} is used for fonts.
     */
    public void startOpenDocument(String purpose, boolean multiple) {
        openPurpose = purpose == null ? "document" : purpose;
        try {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType("picture".equals(openPurpose) ? "image/*" : "*/*");
            if (multiple) {
                intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
            }
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
            startActivityForResult(intent, REQUEST_OPEN);
        } catch (Exception e) {
            Log.e(TAG, "cannot open the document picker", e);
            nativeOpenResult(new byte[0], "", "", message(e));
        }
    }

    /**
     * Writes {@code data} through the Storage Access Framework. A plain document Save reuses the
     * current document's URI; Save As ({@code forcePicker}), exports ({@code setCurrent} false)
     * and unsaved documents open the create-document picker.
     */
    public void startSaveDocument(byte[] data, String name, boolean setCurrent,
                                  boolean forcePicker, long ticket) {
        byte[] bytes = data == null ? new byte[0] : data;
        String safeName = sanitizeName(name);

        String currentUri = currentDocumentUri;
        if (setCurrent && !forcePicker && currentUri != null && !currentUri.isEmpty()) {
            String currentName = currentDocumentName;
            writeTo(Uri.parse(currentUri), bytes, sanitizeName(currentName == null ? safeName : currentName),
                    setCurrent, ticket);
            return;
        }

        if (pendingSave != null) {
            nativeSaveResult(ticket, false, false, safeName, "",
                    "another save is waiting for the file picker");
            return;
        }
        pendingSave = new PendingSave(bytes, safeName, setCurrent, ticket);
        try {
            Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType(mimeFor(safeName));
            intent.putExtra(Intent.EXTRA_TITLE, safeName);
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
            startActivityForResult(intent, REQUEST_SAVE);
        } catch (Exception e) {
            pendingSave = null;
            Log.e(TAG, "cannot open the save picker", e);
            nativeSaveResult(ticket, false, false, safeName, "", message(e));
        }
    }

    // ------------------------------------------------------------------------------------
    // Picker results.
    // ------------------------------------------------------------------------------------

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQUEST_OPEN) {
            handleOpenResult(resultCode, data);
        } else if (requestCode == REQUEST_SAVE) {
            handleSaveResult(resultCode, data);
        }
    }

    private void handleOpenResult(int resultCode, Intent data) {
        if (resultCode != RESULT_OK || data == null) {
            return;
        }
        ArrayList<Uri> uris = new ArrayList<>();
        if (data.getClipData() != null) {
            int count = Math.min(data.getClipData().getItemCount(), MAX_MULTI_FILES);
            for (int i = 0; i < count; i++) {
                Uri uri = data.getClipData().getItemAt(i).getUri();
                if (uri != null) {
                    uris.add(uri);
                }
            }
        } else if (data.getData() != null) {
            uris.add(data.getData());
        }
        if (uris.isEmpty()) {
            return;
        }

        boolean document = "document".equals(openPurpose);
        int flags = data.getFlags()
                & (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        for (Uri uri : uris) {
            takePersistablePermission(uri, flags);
        }

        new Thread(() -> {
            for (Uri uri : uris) {
                String name = displayName(uri);
                try (InputStream in = getContentResolver().openInputStream(uri)) {
                    if (in == null) {
                        throw new IOException("the provider returned no stream");
                    }
                    byte[] bytes = readAll(in);
                    if (document) {
                        currentDocumentUri = uri.toString();
                        currentDocumentName = name;
                    }
                    nativeOpenResult(bytes, name, uri.toString(), "");
                } catch (Exception e) {
                    Log.e(TAG, "cannot read " + uri, e);
                    nativeOpenResult(new byte[0], name, uri.toString(), message(e));
                }
            }
        }, "wordcraft-open").start();
    }

    private void handleSaveResult(int resultCode, Intent data) {
        PendingSave save = pendingSave;
        pendingSave = null;
        if (save == null) {
            return;
        }
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            nativeSaveResult(save.ticket, false, true, save.name, "", "");
            return;
        }
        Uri uri = data.getData();
        takePersistablePermission(uri, data.getFlags()
                & (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION));
        // The provider may have renamed the file to avoid a clash; save under the name it chose.
        String name = displayName(uri);
        writeTo(uri, save.data, name, save.setCurrent, save.ticket);
    }

    private void takePersistablePermission(Uri uri, int flags) {
        int take = flags & (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        if (take == 0) {
            return;
        }
        try {
            getContentResolver().takePersistableUriPermission(uri, take);
        } catch (SecurityException e) {
            // Some providers grant only for the current session; the open/save still works.
            Log.w(TAG, "no persistable permission for " + uri, e);
        }
    }

    private void writeTo(Uri uri, byte[] data, String name, boolean setCurrent, long ticket) {
        new Thread(() -> {
            boolean ok = false;
            String error = "";
            try (OutputStream out = getContentResolver().openOutputStream(uri, "wt")) {
                if (out == null) {
                    throw new IOException("the provider returned no stream");
                }
                out.write(data);
                out.flush();
                ok = true;
            } catch (Exception e) {
                Log.e(TAG, "cannot write " + uri, e);
                error = message(e);
            }
            if (ok && setCurrent) {
                currentDocumentUri = uri.toString();
                currentDocumentName = name;
            }
            nativeSaveResult(ticket, ok, false, name, uri.toString(), error);
        }, "wordcraft-save").start();
    }

    // ------------------------------------------------------------------------------------
    // Names, MIME types and reading.
    // ------------------------------------------------------------------------------------

    private String displayName(Uri uri) {
        String name = null;
        try (Cursor cursor = getContentResolver().query(
                uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (column >= 0) {
                    name = cursor.getString(column);
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "cannot read the display name of " + uri, e);
        }
        if (name == null || name.isEmpty()) {
            name = uri.getLastPathSegment();
        }
        return sanitizeName(name);
    }

    /** A display name safe to hand to the engine: one path component, trimmed and bounded. */
    private static String sanitizeName(String name) {
        if (name == null) {
            return "Document.docx";
        }
        int slash = Math.max(name.lastIndexOf('/'), name.lastIndexOf('\\'));
        String base = slash >= 0 ? name.substring(slash + 1) : name;
        base = base.trim();
        if (base.isEmpty()) {
            return "Document.docx";
        }
        if (base.length() > MAX_NAME_CHARS) {
            base = base.substring(0, MAX_NAME_CHARS);
        }
        return base;
    }

    private static String mimeFor(String name) {
        String n = name.toLowerCase(Locale.ROOT);
        if (n.endsWith(".docx")) {
            return "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        }
        if (n.endsWith(".docm")) {
            return "application/vnd.ms-word.document.macroEnabled.12";
        }
        if (n.endsWith(".dotx")) {
            return "application/vnd.openxmlformats-officedocument.wordprocessingml.template";
        }
        if (n.endsWith(".dotm")) {
            return "application/vnd.ms-word.template.macroEnabled.12";
        }
        if (n.endsWith(".doc")) {
            return "application/msword";
        }
        if (n.endsWith(".pdf")) {
            return "application/pdf";
        }
        if (n.endsWith(".rtf")) {
            return "application/rtf";
        }
        if (n.endsWith(".odt")) {
            return "application/vnd.oasis.opendocument.text";
        }
        if (n.endsWith(".txt")) {
            return "text/plain";
        }
        if (n.endsWith(".md")) {
            return "text/markdown";
        }
        if (n.endsWith(".html") || n.endsWith(".htm")) {
            return "text/html";
        }
        if (n.endsWith(".png")) {
            return "image/png";
        }
        if (n.endsWith(".xml")) {
            return "application/xml";
        }
        return "application/octet-stream";
    }

    private static byte[] readAll(InputStream in) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        byte[] buffer = new byte[64 * 1024];
        long total = 0;
        int read;
        while ((read = in.read(buffer)) != -1) {
            total += read;
            if (total > MAX_OPEN_BYTES) {
                throw new IOException("the file is larger than 128 MiB");
            }
            out.write(buffer, 0, read);
        }
        return out.toByteArray();
    }

    private static String message(Exception e) {
        String text = e.getMessage();
        return text == null || text.isEmpty() ? e.getClass().getSimpleName() : text;
    }

    // Implemented by the native library (apps/wordcraft-android/src/android.rs).
    private static native void nativeOpenResult(
            byte[] data, String name, String uri, String error);

    private static native void nativeSaveResult(
            long ticket, boolean ok, boolean cancelled, String name, String uri, String error);
}
