package org.spindleapp.spindle;

import android.app.NativeActivity;
import android.os.Bundle;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.PopupWindow;
import android.graphics.drawable.ColorDrawable;
import android.view.Gravity;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import android.webkit.WebViewClient;

import java.io.ByteArrayInputStream;
import java.io.FilterInputStream;
import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.Proxy;
import java.net.URL;
import java.util.HashMap;
import java.util.Map;

import java.util.concurrent.atomic.AtomicReference;

public final class SpindleActivity extends NativeActivity implements TextWatcher {
    private WebView htmlPreview;
    private PopupWindow htmlPreviewWindow;
    private String htmlPreviewUrl;
    private boolean handlesBack;
    private android.window.OnBackInvokedCallback backCallback;
    private native void nativeBack();

    public void setBackNavigation(boolean enabled) {
        runOnUiThread(() -> {
            if (handlesBack == enabled || isDestroyed()) return;
            handlesBack = enabled;
            if (android.os.Build.VERSION.SDK_INT >= 33) {
                if (enabled) {
                    backCallback = this::nativeBack;
                    getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                            android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, backCallback);
                } else if (backCallback != null) {
                    getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback(backCallback);
                    backCallback = null;
                }
            }
        });
    }

    @Override
    public void onBackPressed() {
        if (handlesBack) {
            nativeBack();
        } else {
            super.onBackPressed();
        }
    }

    private static final int WEB_LOCATION_PERMISSION = 561;
    private long pendingWebLocation;
    private static native void nativeWebLocation(long id, double latitude, double longitude, double accuracy);

    public void requestWebLocation(long id) {
        runOnUiThread(() -> {
            if (checkSelfPermission(android.Manifest.permission.ACCESS_COARSE_LOCATION)
                    == android.content.pm.PackageManager.PERMISSION_GRANTED) {
                readWebLocation(id);
            } else {
                pendingWebLocation = id;
                requestPermissions(new String[] {android.Manifest.permission.ACCESS_COARSE_LOCATION}, WEB_LOCATION_PERMISSION);
            }
        });
    }

    @Override
    public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(request, permissions, results);
        if (request != WEB_LOCATION_PERMISSION || pendingWebLocation == 0) return;
        long id = pendingWebLocation;
        pendingWebLocation = 0;
        if (results.length > 0 && results[0] == android.content.pm.PackageManager.PERMISSION_GRANTED) {
            readWebLocation(id);
        } else {
            nativeWebLocation(id, Double.NaN, Double.NaN, Double.NaN);
        }
    }

    private void readWebLocation(long id) {
        android.location.LocationManager manager = getSystemService(android.location.LocationManager.class);
        if (manager == null) {
            nativeWebLocation(id, Double.NaN, Double.NaN, Double.NaN);
            return;
        }
        java.util.List<String> providers = manager.getProviders(true);
        String provider = providers.contains("fused") ? "fused" :
            providers.contains(android.location.LocationManager.NETWORK_PROVIDER) ? android.location.LocationManager.NETWORK_PROVIDER :
            providers.contains(android.location.LocationManager.GPS_PROVIDER) ? android.location.LocationManager.GPS_PROVIDER : null;
        if (provider == null) {
            nativeWebLocation(id, Double.NaN, Double.NaN, Double.NaN);
            return;
        }
        final boolean[] finished = {false};
        android.os.Handler timer = new android.os.Handler(getMainLooper());
        android.location.LocationListener listener = new android.location.LocationListener() {
            @Override public void onLocationChanged(android.location.Location position) {
                if (finished[0]) return;
                finished[0] = true;
                manager.removeUpdates(this);
                nativeWebLocation(id, position.getLatitude(), position.getLongitude(), position.getAccuracy());
            }
            @Override public void onProviderEnabled(String name) { }
            @Override public void onProviderDisabled(String name) { }
            @Override public void onStatusChanged(String name, int status, Bundle extras) { }
        };
        try {
            manager.requestLocationUpdates(provider, 0L, 0f, listener, getMainLooper());
            timer.postDelayed(() -> {
                if (finished[0]) return;
                finished[0] = true;
                manager.removeUpdates(listener);
                nativeWebLocation(id, Double.NaN, Double.NaN, Double.NaN);
            }, 12000L);
        } catch (SecurityException | IllegalArgumentException e) {
            nativeWebLocation(id, Double.NaN, Double.NaN, Double.NaN);
        }
    }

    public void showHtmlPreview(String url, String scope, int x, int y, int width, int height) {
        runOnUiThread(() -> {
            if (htmlPreview == null || !url.equals(htmlPreviewUrl)) {
                removeHtmlPreview();
                WebView web = new WebView(this);
                // NativeActivity's EGL surface and WebView's hardware overlay compete on
                // some GPUs. Draw the WebView into its child window's software layer.
                web.setLayerType(View.LAYER_TYPE_SOFTWARE, null);
                android.webkit.WebSettings settings = web.getSettings();
                settings.setJavaScriptEnabled(true);
                settings.setBlockNetworkLoads(true);
                settings.setAllowFileAccess(false);
                settings.setAllowContentAccess(false);
                settings.setGeolocationEnabled(false);
                web.setWebViewClient(new WebViewClient() {
                    @Override
                    public void onReceivedError(WebView view, WebResourceRequest request,
                            android.webkit.WebResourceError error) {
                        if (request.isForMainFrame()) {
                            android.util.Log.w("Spindle", "HTML preview load failed: " + error.getDescription());
                        }
                    }

                    @Override
                    public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                        return !request.getUrl().toString().startsWith(scope);
                    }

                    @Override
                    public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                        String address = request.getUrl().toString();
                        if (!address.startsWith(scope) || !"GET".equals(request.getMethod())) {
                            return unavailablePreviewResource();
                        }
                        return previewResource(address);
                    }
                });
                htmlPreview = web;
                htmlPreviewUrl = url;
                // NativeActivity hands its main surface to Rust. Use a child window for the
                // WebView so Android can draw it above the matching area of the Rust panel.
                PopupWindow preview = new PopupWindow(web, width, height, false);
                preview.setBackgroundDrawable(new ColorDrawable(android.graphics.Color.WHITE));
                preview.setInputMethodMode(PopupWindow.INPUT_METHOD_NOT_NEEDED);
                if (android.os.Build.VERSION.SDK_INT >= 29) {
                    preview.setFocusable(true);
                    preview.setTouchModal(false);
                }
                htmlPreviewWindow = preview;
                preview.showAtLocation(getWindow().getDecorView(), Gravity.TOP | Gravity.LEFT, x, y);
                web.loadUrl(url);
            } else {
                htmlPreviewWindow.update(x, y, width, height);
            }
        });
    }

    public void hideHtmlPreview() {
        runOnUiThread(this::removeHtmlPreview);
    }

    private void removeHtmlPreview() {
        if (htmlPreview == null) return;
        htmlPreviewWindow.dismiss();
        htmlPreviewWindow = null;
        htmlPreview.destroy();
        htmlPreview = null;
        htmlPreviewUrl = null;
    }

    private static WebResourceResponse unavailablePreviewResource() {
        return new WebResourceResponse("text/plain", "UTF-8", 404, "Not Found",
                java.util.Collections.emptyMap(), new ByteArrayInputStream(new byte[0]));
    }

    private static WebResourceResponse previewResource(String address) {
        HttpURLConnection connection = null;
        try {
            connection = (HttpURLConnection) new URL(address).openConnection(Proxy.NO_PROXY);
            connection.setInstanceFollowRedirects(false);
            connection.setConnectTimeout(3000);
            connection.setReadTimeout(5000);
            if (connection.getResponseCode() != 200) {
                connection.disconnect();
                return unavailablePreviewResource();
            }
            String type = connection.getContentType();
            String mime = type.split(";", 2)[0];
            String encoding = type.contains("charset=utf-8") ? "UTF-8" : null;
            Map<String, String> headers = new HashMap<>();
            for (String name : new String[]{"Content-Security-Policy", "Access-Control-Allow-Origin",
                    "Cache-Control", "Referrer-Policy", "X-Content-Type-Options"}) {
                String value = connection.getHeaderField(name);
                if (value != null) headers.put(name, value);
            }
            HttpURLConnection current = connection;
            return new WebResourceResponse(mime, encoding, 200, "OK", headers,
                    new FilterInputStream(current.getInputStream()) {
                        @Override public void close() throws IOException {
                            try { super.close(); } finally { current.disconnect(); }
                        }
                    });
        } catch (IOException error) {
            if (connection != null) connection.disconnect();
            return unavailablePreviewResource();
        }
    }

    @Override
    protected void onDestroy() {
        removeHtmlPreview();
        super.onDestroy();
    }

    public void copyChatSelection(String text) {
        runOnUiThread(() -> {
            android.content.ClipboardManager clipboard =
                (android.content.ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
            clipboard.setPrimaryClip(android.content.ClipData.newPlainText("Spindle", text));
        });
    }

    public String buildApk(String request) {
        return org.spindleapp.spindle.builder.ApkBuilder.build(this, request);
    }

    public String openUpdateApk(String version) {
        try {
            java.io.File apk = UpdateApkProvider.file(this, version);
            android.content.pm.PackageManager packages = getPackageManager();
            android.content.pm.PackageInfo update = packages.getPackageArchiveInfo(apk.getPath(), 0);
            if (update == null || !getPackageName().equals(update.packageName)) {
                return "This download is not a Spindle APK.";
            }
            android.content.pm.PackageInfo installed = packages.getPackageInfo(getPackageName(), 0);
            if (apkVersion(update) <= apkVersion(installed)) {
                return "This APK build is already installed or older than your current build.";
            }
        } catch (IOException | android.content.pm.PackageManager.NameNotFoundException error) {
            return "Android could not inspect the downloaded APK.";
        }
        java.util.concurrent.FutureTask<String> open = new java.util.concurrent.FutureTask<>(() -> {
            android.content.Intent intent = new android.content.Intent(android.content.Intent.ACTION_VIEW)
                    .setDataAndType(UpdateApkProvider.uri(this, version), UpdateApkProvider.MIME)
                    .addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION);
            try {
                startActivity(intent);
                return "";
            } catch (android.content.ActivityNotFoundException error) {
                return "Android’s package installer is unavailable.";
            } catch (SecurityException error) {
                return "Android blocked the installer. Allow installs from Spindle in Android Settings and try again.";
            }
        });
        runOnUiThread(open);
        try {
            return open.get(5, java.util.concurrent.TimeUnit.SECONDS);
        } catch (Exception error) {
            open.cancel(false);
            return "Android could not open the package installer.";
        }
    }

    private static long apkVersion(android.content.pm.PackageInfo info) {
        return android.os.Build.VERSION.SDK_INT >= 28 ? info.getLongVersionCode() : info.versionCode;
    }

    public String downloadApk(String name, byte[] bytes) throws Exception {
        if (!name.matches("[a-zA-Z0-9][a-zA-Z0-9_.-]{0,95}[.](apk|exe|elf)")) {
            throw new IllegalArgumentException("Invalid executable download name");
        }
        if (android.os.Build.VERSION.SDK_INT < 29) {
            java.io.File folder = getExternalFilesDir(android.os.Environment.DIRECTORY_DOWNLOADS);
            if (folder == null) throw new java.io.IOException("Downloads storage is unavailable");
            java.io.File file = new java.io.File(folder, name);
            java.nio.file.Files.write(file.toPath(), bytes, java.nio.file.StandardOpenOption.CREATE_NEW);
            return file.getAbsolutePath();
        }
        android.content.ContentResolver resolver = getContentResolver();
        android.content.ContentValues values = new android.content.ContentValues();
        values.put(android.provider.MediaStore.Downloads.DISPLAY_NAME, name);
        values.put(android.provider.MediaStore.Downloads.MIME_TYPE, name.endsWith(".apk") ? "application/vnd.android.package-archive" : "application/octet-stream");
        values.put(android.provider.MediaStore.Downloads.RELATIVE_PATH, android.os.Environment.DIRECTORY_DOWNLOADS);
        values.put(android.provider.MediaStore.Downloads.IS_PENDING, 1);
        android.net.Uri uri = resolver.insert(android.provider.MediaStore.Downloads.EXTERNAL_CONTENT_URI, values);
        if (uri == null) throw new java.io.IOException("Could not create the APK download");
        try {
            try (java.io.OutputStream output = resolver.openOutputStream(uri)) {
                if (output == null) throw new java.io.IOException("Could not open the APK download");
                output.write(bytes);
            }
            values.clear();
            values.put(android.provider.MediaStore.Downloads.IS_PENDING, 0);
            resolver.update(uri, values, null, null);
            try (android.database.Cursor row = resolver.query(uri,
                    new String[]{android.provider.MediaStore.Downloads.DISPLAY_NAME}, null, null, null)) {
                if (row != null && row.moveToFirst()) name = row.getString(0);
            }
            return "Downloads/" + name;
        } catch (Exception error) {
            resolver.delete(uri, null, null);
            throw error;
        }
    }

    // This key belongs to the installed app and stays in Android Keystore.
    private static final String PROVIDER_KEY = "spindle-provider-credentials-v1";

    public synchronized byte[] protectProviderCredential(byte[] input, boolean encrypt)
            throws Exception {
        java.security.KeyStore store = java.security.KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        javax.crypto.SecretKey key = (javax.crypto.SecretKey) store.getKey(PROVIDER_KEY, null);
        if (encrypt && key == null) {
            javax.crypto.KeyGenerator generator = javax.crypto.KeyGenerator.getInstance(
                android.security.keystore.KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
            generator.init(new android.security.keystore.KeyGenParameterSpec.Builder(PROVIDER_KEY,
                android.security.keystore.KeyProperties.PURPOSE_ENCRYPT |
                android.security.keystore.KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(android.security.keystore.KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(android.security.keystore.KeyProperties.ENCRYPTION_PADDING_NONE)
                .build());
            key = generator.generateKey();
        }
        if (key == null) throw new java.security.KeyStoreException("Credential key is unavailable");
        javax.crypto.Cipher cipher = javax.crypto.Cipher.getInstance("AES/GCM/NoPadding");
        if (!encrypt) {
            if (input.length < 28) throw new IllegalArgumentException("Invalid credential");
            cipher.init(javax.crypto.Cipher.DECRYPT_MODE, key,
                new javax.crypto.spec.GCMParameterSpec(128, input, 0, 12));
            return cipher.doFinal(input, 12, input.length - 12);
        }
        cipher.init(javax.crypto.Cipher.ENCRYPT_MODE, key);
        byte[] iv = cipher.getIV();
        if (iv.length != 12) throw new java.security.GeneralSecurityException("Unexpected IV length");
        byte[] payload = cipher.doFinal(input);
        byte[] sealed = java.util.Arrays.copyOf(iv, iv.length + payload.length);
        System.arraycopy(payload, 0, sealed, iv.length, payload.length);
        return sealed;
    }

    // Rust draws the text fields. The EditText supplies the Android input connection.
    private EditText editor;
    private View nativeView;
    private int revision;
    private int editorMode;
    private boolean applying;
    private final AtomicReference<String[]> snapshot = new AtomicReference<>();

    private static final class ComposerEditor extends EditText {
        private final SpindleActivity activity;

        ComposerEditor(SpindleActivity activity) {
            super(activity);
            this.activity = activity;
        }

        @Override
        protected void onSelectionChanged(int start, int end) {
            super.onSelectionChanged(start, end);
            if (activity != null) activity.publish();
        }
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        ViewGroup content = findViewById(android.R.id.content);
        nativeView = content.getChildAt(0);
        editor = new ComposerEditor(this);
        editor.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE
                | InputType.TYPE_TEXT_FLAG_CAP_SENTENCES | InputType.TYPE_TEXT_FLAG_AUTO_CORRECT);
        editor.setImeOptions(EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_FULLSCREEN);
        editor.setAlpha(0);
        editor.setSaveEnabled(false);
        editor.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        editor.setVisibility(View.INVISIBLE);
        editor.addTextChangedListener(this);
        content.addView(editor, new ViewGroup.LayoutParams(1, 1));
    }

    @Override
    public void beforeTextChanged(CharSequence text, int start, int count, int after) {}

    @Override
    public void onTextChanged(CharSequence text, int start, int before, int count) {}

    @Override
    public void afterTextChanged(Editable text) {
        publish();
    }

    private void publish() {
        if (editor != null && revision != 0 && !applying) {
            snapshot.set(new String[] {Integer.toString(revision), editor.getText().toString(),
                    Integer.toString(editor.getSelectionStart()), Integer.toString(editor.getSelectionEnd())});
        }
    }

    // Called from the render thread. View and Editable operations run on Android's UI thread.
    public void setComposerIme(int version, String text, int start, int end, boolean show, int mode) {
        runOnUiThread(() -> {
            if (editor == null) return;
            boolean opening = editor.getVisibility() != View.VISIBLE;
            revision = version;
            applying = true;
            try {
                if (mode != editorMode) {
                    editorMode = mode;
                    editor.setSingleLine(mode != 0);
                    int inputType = InputType.TYPE_CLASS_TEXT;
                    if (mode == 0) {
                        inputType |= InputType.TYPE_TEXT_FLAG_MULTI_LINE
                                | InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
                                | InputType.TYPE_TEXT_FLAG_AUTO_CORRECT;
                    } else if (mode == 2) {
                        inputType |= InputType.TYPE_TEXT_VARIATION_PASSWORD
                                | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
                    }
                    editor.setInputType(inputType);
                    editor.setImeOptions(EditorInfo.IME_FLAG_NO_EXTRACT_UI
                            | EditorInfo.IME_FLAG_NO_FULLSCREEN
                            | (mode == 2 ? EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING : 0));
                }
                if (!text.contentEquals(editor.getText())) editor.setText(text);
                int len = editor.length();
                editor.setSelection(Math.max(0, Math.min(start, len)), Math.max(0, Math.min(end, len)));
                if (opening) editor.setVisibility(View.VISIBLE);
                if (!editor.hasFocus()) editor.requestFocus();
                if (opening || show) {
                    editor.post(() -> {
                        if (revision != 0 && editor.hasFocus()) {
                            getSystemService(InputMethodManager.class)
                                    .showSoftInput(editor, InputMethodManager.SHOW_IMPLICIT);
                        }
                    });
                }
            } finally {
                applying = false;
            }
        });
    }

    public String[] takeComposerIme() {
        return snapshot.getAndSet(null);
    }

    public void closeComposerIme() {
        runOnUiThread(() -> {
            if (editor == null || editor.getVisibility() != View.VISIBLE) return;
            revision = 0;
            getSystemService(InputMethodManager.class)
                    .hideSoftInputFromWindow(editor.getWindowToken(), 0);
            editor.clearFocus();
            editor.setVisibility(View.INVISIBLE);
            editor.setText("");
            snapshot.set(null);
            if (nativeView != null) nativeView.requestFocus();
        });
    }
}
