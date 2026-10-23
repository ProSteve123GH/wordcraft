package com.storytold.wordcraft;

import android.content.Intent;
import android.content.SharedPreferences;
import android.content.UriPermission;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.graphics.Color;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Process;
import android.provider.OpenableColumns;
import android.util.Log;
import android.view.View;
import android.window.OnBackInvokedDispatcher;
import android.widget.Toast;

import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;

import com.google.androidgamesdk.GameActivity;

import java.io.ByteArrayOutputStream;
import java.io.File;
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
    /** Longest status message handed back to the engine. */
    private static final int MAX_NOTICE_CHARS = 300;

    /** Private storage for the document last opened or saved, so it survives process death. */
    private static final String PREFS_DOCUMENT = "wordcraft.document";
    private static final String KEY_DOCUMENT_URI = "document.uri";
    private static final String KEY_DOCUMENT_NAME = "document.name";

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
        restoreCurrentDocument();
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        installInsetsListener();
        installBackCallback();
        hideSystemBars();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideSystemBars();
        }
    }

    /**
     * Reports the system bars and display cutout to Rust, in device-independent pixels. The Rust
     * host puts them in {@code RawInput::safe_area_insets}, so the ribbon, page and status bar
     * never sit under the cutout, the status bar or the navigation/gesture area. IME visibility
     * lets Back hide the keyboard before it goes on to the document.
     */
    private void installInsetsListener() {
        View decor = getWindow().getDecorView();
        ViewCompat.setOnApplyWindowInsetsListener(decor, (view, windowInsets) -> {
            Insets bars = windowInsets.getInsets(
                    WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
            float density = getResources().getDisplayMetrics().density;
            if (!(density > 0f)) {
                density = 1f;
            }
            nativeInsets(bars.left / density, bars.top / density,
                    bars.right / density, bars.bottom / density);
            nativeImeVisible(windowInsets.isVisible(WindowInsetsCompat.Type.ime()));
            return windowInsets;
        });
        ViewCompat.requestApplyInsets(decor);
    }

    /**
     * Back goes to Rust: it hides the keyboard first, then closes the nearest dialog or page, and
     * only then asks to close the document (Save Changes included). API 33+ uses the predictive
     * back dispatcher; older releases deliver {@code onBackPressed}.
     */
    private void installBackCallback() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                    OnBackInvokedDispatcher.PRIORITY_DEFAULT, this::requestBack);
        }
    }

    private void requestBack() {
        nativeBack();
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        requestBack();
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
    protected void onResume() {
        super.onResume();
        // The bars may have been shown while another app was in front: hide them again and ask
        // for fresh insets (the activity's size or orientation may also have changed).
        hideSystemBars();
        ViewCompat.requestApplyInsets(getWindow().getDecorView());
        nativeResume();
    }

    @Override
    protected void onPause() {
        // Settings are written now; the UI thread may not run another frame before the activity
        // is suspended, and a later destroy or process death must not lose them.
        nativePause();
        super.onPause();
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
            Uri uri = parseDocumentUri(currentUri);
            if (uri != null && canWrite(uri)) {
                String currentName = currentDocumentName;
                writeTo(uri, bytes, sanitizeName(currentName == null ? safeName : currentName),
                        setCurrent, ticket, false);
                return;
            }
            // A permission can be revoked, or the provider can remove the file, while the app is
            // gone. Forget it and fall back to the save picker instead of failing silently.
            Log.w(TAG, "the current document is no longer writable; asking where to save");
            forgetCurrentDocument();
            notifyStorage("The document's file can't be written with the current access. Choose where to save it.");
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

    /**
     * Reopens a document the user picked in File › Home's Recent list. The URI is the one stored
     * in the app's private preferences; its persistable permission was taken when the document
     * was first opened or saved. A revoked permission is reported to the native side, which shows
     * the error instead of crashing.
     */
    public void startOpenUri(String uriText) {
        openPurpose = "document";
        Uri uri = parseDocumentUri(uriText);
        if (uri == null) {
            nativeOpenResult(new byte[0], "", "", "the recent document's location is no longer valid");
            return;
        }
        // Best effort: take the grant the system already keeps for this URI. Taking flags the
        // provider never granted would make the call throw and renew nothing, so ask only for
        // the modes in the persisted permission.
        int persisted = persistedFlags(uri);
        if (persisted != 0) {
            takePersistablePermission(uri, persisted);
        }
        if (!canRead(uri)) {
            nativeOpenResult(new byte[0], "", uri.toString(),
                    "access to this document has expired; open it again with Open");
            return;
        }
        new Thread(() -> readOpenUri(uri, true), "wordcraft-open").start();
    }

    /** A new document replaced the old one: Save must ask where to write from now on. */
    public void forgetCurrentDocument() {
        currentDocumentUri = null;
        currentDocumentName = null;
        try {
            getSharedPreferences(PREFS_DOCUMENT, MODE_PRIVATE).edit()
                    .remove(KEY_DOCUMENT_URI)
                    .remove(KEY_DOCUMENT_NAME)
                    .apply();
        } catch (Exception e) {
            Log.w(TAG, "cannot forget the current document", e);
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
        int flags = grantFlags(data);
        for (Uri uri : uris) {
            takePersistablePermission(uri, flags);
        }

        new Thread(() -> {
            for (Uri uri : uris) {
                readOpenUri(uri, document);
            }
        }, "wordcraft-open").start();
    }

    /** Reads one picked or recent URI on a worker thread and reports it to the native side. */
    private void readOpenUri(Uri uri, boolean document) {
        String name = displayName(uri);
        try (InputStream in = getContentResolver().openInputStream(uri)) {
            if (in == null) {
                throw new IOException("the provider returned no stream");
            }
            byte[] bytes = readAll(in);
            if (document) {
                setCurrentDocument(uri, name);
            }
            nativeOpenResult(bytes, name, uri.toString(), "");
        } catch (Exception e) {
            Log.e(TAG, "cannot read " + uri, e);
            nativeOpenResult(new byte[0], name, uri.toString(), message(e));
        }
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
        takePersistablePermission(uri, grantFlags(data));
        // The provider may have renamed the file to avoid a clash; save under the name it chose.
        String name = displayName(uri);
        writeTo(uri, save.data, name, save.setCurrent, save.ticket, true);
    }

    /**
     * The read/write modes a picker result granted. When the result carries no flags at all,
     * ask for both: taking a mode the provider never granted is caught, while a picker that
     * simply didn't report flags would otherwise lose its grant at the next launch.
     */
    private static int grantFlags(Intent data) {
        int flags = data == null ? 0
                : data.getFlags() & (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        return flags == 0 ? (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION) : flags;
    }

    private void takePersistablePermission(Uri uri, int flags) {
        int take = flags & (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        if (take == 0 || uri == null || !"content".equals(uri.getScheme())) {
            return;
        }
        try {
            getContentResolver().takePersistableUriPermission(uri, take);
        } catch (Exception e) {
            // Some providers grant only for the current session; the open/save still works.
            Log.w(TAG, "no persistable permission for " + uri, e);
        }
    }

    /** The read/write flags the system still keeps for {@code uri}, or 0 when it holds none. */
    private int persistedFlags(Uri uri) {
        if (uri == null || !"content".equals(uri.getScheme())) {
            return 0;
        }
        int flags = 0;
        try {
            for (UriPermission permission : getContentResolver().getPersistedUriPermissions()) {
                if (permission == null || !uri.equals(permission.getUri())) {
                    continue;
                }
                if (permission.isReadPermission()) {
                    flags |= Intent.FLAG_GRANT_READ_URI_PERMISSION;
                }
                if (permission.isWritePermission()) {
                    flags |= Intent.FLAG_GRANT_WRITE_URI_PERMISSION;
                }
                break;
            }
        } catch (Exception e) {
            Log.w(TAG, "cannot read the persisted permissions", e);
        }
        return flags;
    }

    /**
     * Re-take and verify the permission the last document was opened or saved with. False means
     * the grant is gone (revoked, or the provider removed the file): the caller forgets the
     * document and asks the user to choose it again instead of failing on the next Save. A
     * provider that grants only reading still restores the document; its next Save falls back to
     * the picker with a message (see startSaveDocument).
     */
    private boolean renewCurrentPermission(Uri uri) {
        if ("file".equals(uri.getScheme())) {
            return canRead(uri);
        }
        int flags = persistedFlags(uri);
        if (flags != 0) {
            takePersistablePermission(uri, flags);
        } else if (!canRead(uri) && !canWrite(uri)) {
            // No persisted grant and no live one: after a restart the file is out of reach.
            Log.w(TAG, "no persisted permission for the last document " + uri);
            return false;
        }
        boolean readable = canRead(uri);
        boolean writable = canWrite(uri);
        if (!readable && !writable) {
            Log.w(TAG, "the last document is no longer accessible " + uri);
            return false;
        }
        if (!readable) {
            // Rare, but a write-only grant still lets Save keep the file up to date.
            Log.w(TAG, "the last document is write-only " + uri);
        }
        if (!writable) {
            // Read-only grants are common for cloud providers: keep the document, but Save will
            // ask where (with a message) instead of failing.
            Log.w(TAG, "the last document is read-only " + uri);
        }
        return true;
    }

    /** The document last opened or saved, read back after process death. Junk is forgotten. */
    private void restoreCurrentDocument() {
        try {
            SharedPreferences prefs = getSharedPreferences(PREFS_DOCUMENT, MODE_PRIVATE);
            String uri = prefs.getString(KEY_DOCUMENT_URI, "");
            String name = prefs.getString(KEY_DOCUMENT_NAME, "");
            if (uri == null || uri.isEmpty()) {
                return;
            }
            Uri parsed = parseDocumentUri(uri);
            if (parsed == null) {
                Log.w(TAG, "forgetting the saved document: its location is not supported");
                forgetCurrentDocument();
                return;
            }
            currentDocumentUri = uri;
            currentDocumentName = sanitizeName(name);
            // A URI without its grant is worse than no URI: Save would fail with EACCES/ENOENT.
            // Verify it now, and forget it so the next Save asks where instead.
            if (!renewCurrentPermission(parsed)) {
                Log.w(TAG, "forgetting the saved document: access to " + uri + " has expired");
                String shown = currentDocumentName;
                forgetCurrentDocument();
                notifyStorage("The last document (" + shown
                        + ") can't be reopened because access has expired. Choose it again with File > Open.");
                return;
            }
            Log.i(TAG, "restored the current document " + currentDocumentName);
        } catch (Exception e) {
            Log.w(TAG, "cannot restore the current document", e);
        }
    }

    /** Record the document Save writes without asking again, in memory and private storage. */
    private void setCurrentDocument(Uri uri, String name) {
        if (uri == null) {
            return;
        }
        String text = uri.toString();
        if (text.isEmpty()) {
            return;
        }
        String safeName = sanitizeName(name);
        currentDocumentUri = text;
        currentDocumentName = safeName;
        try {
            getSharedPreferences(PREFS_DOCUMENT, MODE_PRIVATE).edit()
                    .putString(KEY_DOCUMENT_URI, text)
                    .putString(KEY_DOCUMENT_NAME, safeName)
                    .apply();
        } catch (Exception e) {
            // The document still saves for this run; only the relaunch record is lost.
            Log.w(TAG, "cannot remember the current document", e);
        }
    }

    /** A content: or file: URI, or null for anything else (never throws). */
    private static Uri parseDocumentUri(String text) {
        if (text == null || text.isEmpty()) {
            return null;
        }
        try {
            Uri uri = Uri.parse(text);
            String scheme = uri.getScheme();
            if ("content".equals(scheme) || "file".equals(scheme)) {
                return uri;
            }
        } catch (Exception e) {
            Log.w(TAG, "cannot parse a document location", e);
        }
        return null;
    }

    private boolean canRead(Uri uri) {
        return canUse(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION);
    }

    private boolean canWrite(Uri uri) {
        return canUse(uri, Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
    }

    /**
     * Whether this app currently holds {@code mode} for {@code uri}. A failed check says yes and
     * lets the read or write report the real problem; a plain file URI is handled by the stream.
     */
    private boolean canUse(Uri uri, int mode) {
        String scheme = uri.getScheme();
        if ("file".equals(scheme)) {
            // A plain path has no SAF grant; the file itself has to exist and allow the mode.
            String path = uri.getPath();
            if (path == null || path.isEmpty()) {
                return false;
            }
            File file = new File(path);
            return mode == Intent.FLAG_GRANT_READ_URI_PERMISSION ? file.canRead() : file.canWrite();
        }
        if (!"content".equals(scheme)) {
            return false;
        }
        try {
            return checkUriPermission(uri, Process.myPid(), Process.myUid(), mode)
                    == PackageManager.PERMISSION_GRANTED;
        } catch (Exception e) {
            Log.w(TAG, "cannot check access to " + uri, e);
            return true;
        }
    }

    /**
     * Writes {@code data} to {@code uri} on a worker thread. {@code fromPicker} separates a file
     * the user just chose from the document's own URI: when writing the latter fails (the file
     * was deleted, its grant revoked, the provider moved it), the save falls back to the picker
     * with a message instead of reporting a failure the user can't act on.
     */
    private void writeTo(Uri uri, byte[] data, String name, boolean setCurrent, long ticket,
                         boolean fromPicker) {
        new Thread(() -> {
            boolean ok = false;
            String error = "";
            try {
                try (OutputStream out = getContentResolver().openOutputStream(uri, "wt")) {
                    if (out == null) {
                        throw new IOException("the provider returned no stream");
                    }
                    out.write(data);
                    out.flush();
                }
                // Only a write whose stream also closed cleanly counts: providers flush to disk
                // or the cloud on close, so an exception there means the save did not happen.
                ok = true;
            } catch (Exception e) {
                Log.e(TAG, "cannot write " + uri, e);
                error = message(e);
            }
            if (ok && setCurrent) {
                setCurrentDocument(uri, name);
            }
            if (ok || fromPicker || !setCurrent) {
                nativeSaveResult(ticket, ok, false, name, uri.toString(), error);
                return;
            }
            // The document's own URI no longer works: forget it and ask where to save instead.
            final String reason = error;
            final byte[] retry = data;
            final String retryName = name;
            runOnUiThread(() -> {
                forgetCurrentDocument();
                notifyStorage("The document's file couldn't be written (" + reason
                        + "). Choose where to save it.");
                startSaveDocument(retry, retryName, setCurrent, true, ticket);
            });
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

    /**
     * Put a storage message on WordCraft's status bar (and show a toast, so it is noticed even
     * before the next frame). Called on the Java main thread; a failure to reach either side is
     * logged, never thrown: this app must not crash.
     */
    private void notifyStorage(String message) {
        if (message == null) {
            return;
        }
        String text = message.trim();
        if (text.codePointCount(0, text.length()) > MAX_NOTICE_CHARS) {
            // Count code points so a long provider message can't be cut through a surrogate pair.
            text = text.substring(0, text.offsetByCodePoints(0, MAX_NOTICE_CHARS));
        }
        Log.i(TAG, "storage: " + text);
        try {
            nativeStorageNotice(text);
        } catch (Throwable t) {
            Log.w(TAG, "cannot report a storage message to the native side", t);
        }
        try {
            Toast.makeText(this, text, Toast.LENGTH_LONG).show();
        } catch (Throwable t) {
            Log.w(TAG, "cannot show a storage message", t);
        }
    }

    // Implemented by the native library (apps/wordcraft-android/src/android.rs).
    private static native void nativeOpenResult(
            byte[] data, String name, String uri, String error);

    private static native void nativeSaveResult(
            long ticket, boolean ok, boolean cancelled, String name, String uri, String error);

    private static native void nativeInsets(
            float left, float top, float right, float bottom);

    private static native void nativeImeVisible(boolean visible);

    private static native void nativeBack();

    private static native void nativePause();

    private static native void nativeResume();

    private static native void nativeStorageNotice(String message);
}
