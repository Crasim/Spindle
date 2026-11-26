package org.spindleapp.spindle.builder;

import android.content.Context;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import com.android.apksig.ApkSigner;
import com.android.apksig.ApkVerifier;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.Collections;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

/** Package a fixed binary manifest and the worker's DEX, then sign with a local debug key. */
final class FixtureApk {
    private static final String KEY = "spindle-build-probe-debug";

    static synchronized File build(Context context, byte[] dex) throws Exception {
        File cache = context.getCacheDir();
        File unsigned = File.createTempFile("fixture-", ".unsigned", cache);
        try {
            File signed = File.createTempFile("fixture-", ".signed", cache);
            try {
                try (ZipOutputStream apk = new ZipOutputStream(new FileOutputStream(unsigned));
                     InputStream manifest = context.getAssets().open("fixture-manifest.bin")) {
                    apk.putNextEntry(new ZipEntry("AndroidManifest.xml"));
                    byte[] buffer = new byte[8192];
                    for (int count; (count = manifest.read(buffer)) != -1; ) {
                        apk.write(buffer, 0, count);
                    }
                    apk.closeEntry();
                    apk.putNextEntry(new ZipEntry("classes.dex"));
                    apk.write(dex);
                    apk.closeEntry();
                }
                KeyStore store = KeyStore.getInstance("AndroidKeyStore");
                store.load(null);
                if (!store.containsAlias(KEY)) {
                    KeyPairGenerator keys = KeyPairGenerator.getInstance("RSA", "AndroidKeyStore");
                    keys.initialize(new KeyGenParameterSpec.Builder(KEY,
                            KeyProperties.PURPOSE_SIGN | KeyProperties.PURPOSE_VERIFY)
                            .setKeySize(2048).setDigests(KeyProperties.DIGEST_SHA256)
                            .setSignaturePaddings(KeyProperties.SIGNATURE_PADDING_RSA_PKCS1).build());
                    keys.generateKeyPair();
                }
                ApkSigner.SignerConfig signer = new ApkSigner.SignerConfig.Builder("fixture",
                        (PrivateKey) store.getKey(KEY, null),
                        Collections.singletonList((X509Certificate) store.getCertificate(KEY))).build();
                new ApkSigner.Builder(Collections.singletonList(signer))
                        .setInputApk(unsigned).setOutputApk(signed).setMinSdkVersion(26)
                        .setV1SigningEnabled(false).setV2SigningEnabled(true)
                        .setV3SigningEnabled(false).setV4SigningEnabled(false).build().sign();
                ApkVerifier.Result verification = new ApkVerifier.Builder(signed).build().verify();
                if (!verification.isVerified() || !verification.isVerifiedUsingV2Scheme()) {
                    throw new IllegalStateException("Fixture APK signature failed verification.");
                }
                File output = new File(cache, "SpindleJavaFixture.apk");
                Files.move(signed.toPath(), output.toPath(), StandardCopyOption.ATOMIC_MOVE,
                        StandardCopyOption.REPLACE_EXISTING);
                return output;
            } finally {
                Files.deleteIfExists(signed.toPath());
            }
        } finally {
            Files.deleteIfExists(unsigned.toPath());
        }
    }

    private FixtureApk() {}
}
