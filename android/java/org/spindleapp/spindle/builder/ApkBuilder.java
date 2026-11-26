package org.spindleapp.spindle.builder;

import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.Message;
import android.os.Messenger;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import com.android.apksig.ApkSigner;
import com.android.apksig.ApkVerifier;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.nio.file.Files;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.Collections;
import java.util.Iterator;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import org.json.JSONObject;

public final class ApkBuilder {
    public static String build(Context context, String input) {
        try {
            JSONObject request = new JSONObject(input);
            String packageName = request.getString("package");
            String activity = request.getString("activity");
            String label = request.getString("label");
            JSONObject sources = request.getJSONObject("sources");
            byte[] dex = compile(context.getApplicationContext(), activity, sources);
            byte[] apk = sign(context, packageName, activity, label, dex);
            return new JSONObject().put("apk", Base64.encodeToString(apk, Base64.NO_WRAP)).toString();
        } catch (Exception | LinkageError failure) {
            Throwable cause = failure.getCause() != null ? failure.getCause() : failure;
            String detail = cause.getMessage();
            if (detail == null) detail = cause.getClass().getSimpleName();
            try {
                return new JSONObject().put("error", detail.substring(0, Math.min(detail.length(), 4096)))
                        .toString();
            } catch (org.json.JSONException impossible) {
                return "{\"error\":\"APK build failed\"}";
            }
        }
    }

    private static byte[] compile(Context context, String activity, JSONObject sources) throws Exception {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            throw new IllegalStateException("APK compilation must run off the Android UI thread.");
        }
        String[] paths = new String[sources.length()];
        String[] contents = new String[sources.length()];
        int index = 0;
        for (Iterator<String> keys = sources.keys(); keys.hasNext(); index++) {
            paths[index] = keys.next();
            contents[index] = sources.getString(paths[index]);
        }
        CompletableFuture<byte[]> finished = new CompletableFuture<>();
        Messenger reply = new Messenger(new Handler(Looper.getMainLooper(), message -> {
            if (message.what != ApkBuildService.RESULT) return true;
            Bundle result = message.getData();
            byte[] dex = result.getByteArray("dex");
            if (dex == null || dex.length == 0 || dex.length > ApkBuildService.MAX_DEX_BYTES) {
                finished.completeExceptionally(new IllegalArgumentException(
                        result.getString("error", "No DEX was returned.")));
            } else {
                finished.complete(dex);
            }
            return true;
        }));
        ServiceConnection connection = new ServiceConnection() {
            @Override public void onServiceConnected(ComponentName name, IBinder binder) {
                try {
                    Bundle data = new Bundle();
                    data.putString("activity", activity);
                    data.putStringArray("paths", paths);
                    data.putStringArray("contents", contents);
                    Message message = Message.obtain(null, ApkBuildService.COMPILE);
                    message.setData(data);
                    message.replyTo = reply;
                    new Messenger(binder).send(message);
                } catch (Exception failure) {
                    finished.completeExceptionally(failure);
                }
            }
            @Override public void onServiceDisconnected(ComponentName name) {
                finished.completeExceptionally(new IllegalStateException("The APK compiler stopped."));
            }
            @Override public void onBindingDied(ComponentName name) {
                onServiceDisconnected(name);
            }
            @Override public void onNullBinding(ComponentName name) {
                onServiceDisconnected(name);
            }
        };
        boolean bound = context.bindService(new Intent(context, ApkBuildService.class), connection,
                Context.BIND_AUTO_CREATE);
        if (!bound) throw new IllegalStateException("Android could not start the isolated APK compiler.");
        try {
            return finished.get(45, TimeUnit.SECONDS);
        } catch (java.util.concurrent.TimeoutException timeout) {
            throw new IllegalStateException("The isolated APK compiler timed out after 45 seconds.");
        } finally {
            context.unbindService(connection);
        }
    }

    private static byte[] sign(Context context, String packageName, String activity, String label, byte[] dex)
            throws Exception {
        byte[] template;
        try (InputStream stream = context.getAssets().open("apk-builder/manifest.bin")) {
            java.io.ByteArrayOutputStream buffer = new java.io.ByteArrayOutputStream();
            byte[] chunk = new byte[2048];
            for (int count; (count = stream.read(chunk)) != -1; ) {
                if (buffer.size() + count > 8192) throw new IllegalStateException("APK template exceeds the size limit.");
                buffer.write(chunk, 0, count);
            }
            template = buffer.toByteArray();
        }
        byte[] manifest = ApkManifest.render(template, packageName, activity, label);
        File unsigned = File.createTempFile("spindle-build-", ".unsigned", context.getCacheDir());
        try {
            File signed = File.createTempFile("spindle-build-", ".apk", context.getCacheDir());
            try {
                try (ZipOutputStream zip = new ZipOutputStream(new FileOutputStream(unsigned))) {
                    entry(zip, "AndroidManifest.xml", manifest);
                    entry(zip, "classes.dex", dex);
                }
                ApkSigner.SignerConfig key = localKey(packageName);
                new ApkSigner.Builder(Collections.singletonList(key))
                        .setInputApk(unsigned).setOutputApk(signed).setMinSdkVersion(26)
                        .setV1SigningEnabled(false).setV2SigningEnabled(true)
                        .setV3SigningEnabled(false).setV4SigningEnabled(false).build().sign();
                ApkVerifier.Result verified = new ApkVerifier.Builder(signed).build().verify();
                if (!verified.isVerified() || !verified.isVerifiedUsingV2Scheme()) {
                    throw new IllegalStateException("The generated APK signature did not verify.");
                }
                return Files.readAllBytes(signed.toPath());
            } finally {
                Files.deleteIfExists(signed.toPath());
            }
        } finally {
            Files.deleteIfExists(unsigned.toPath());
        }
    }

    private static synchronized ApkSigner.SignerConfig localKey(String packageName) throws Exception {
        String alias = "spindle-built-apk-v1:" + packageName;
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        if (!store.containsAlias(alias)) {
            KeyPairGenerator keys = KeyPairGenerator.getInstance("RSA", "AndroidKeyStore");
            keys.initialize(new KeyGenParameterSpec.Builder(alias,
                    KeyProperties.PURPOSE_SIGN | KeyProperties.PURPOSE_VERIFY)
                    .setKeySize(2048).setDigests(KeyProperties.DIGEST_SHA256)
                    .setSignaturePaddings(KeyProperties.SIGNATURE_PADDING_RSA_PKCS1).build());
            keys.generateKeyPair();
        }
        return new ApkSigner.SignerConfig.Builder("debug",
                (PrivateKey) store.getKey(alias, null),
                Collections.singletonList((X509Certificate) store.getCertificate(alias))).build();
    }

    private static void entry(ZipOutputStream zip, String name, byte[] data) throws Exception {
        zip.putNextEntry(new ZipEntry(name));
        zip.write(data);
        zip.closeEntry();
    }

    private ApkBuilder() {}
}
