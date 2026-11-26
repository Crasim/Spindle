package org.spindleapp.spindle.builder;

import android.Manifest;
import android.app.Service;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.Message;
import android.os.Messenger;
import android.os.Process;
import android.os.RemoteException;
import java.io.InputStream;
import java.util.LinkedHashMap;
import java.util.Map;

public final class ApkBuildService extends Service {
    static final int COMPILE = 1;
    static final int RESULT = 2;
    static final int MAX_DEX_BYTES = 384 * 1024;
    private boolean compiling;

    private final Messenger endpoint = new Messenger(new Handler(Looper.getMainLooper(), message -> {
        if (message.what != COMPILE || message.replyTo == null || compiling) return true;
        compiling = true;
        Bundle request = message.getData();
        Messenger reply = message.replyTo;
        new Thread(() -> compile(request, reply), "spindle-apk-compiler").start();
        return true;
    }));

    @Override public IBinder onBind(Intent intent) {
        return endpoint.getBinder();
    }

    private void compile(Bundle request, Messenger reply) {
        Bundle result = new Bundle();
        try {
            if (Process.myUid() == getApplicationInfo().uid ||
                    checkSelfPermission(Manifest.permission.INTERNET) != PackageManager.PERMISSION_DENIED) {
                throw new IllegalStateException("Compiler isolation check failed.");
            }
            String[] paths = request.getStringArray("paths");
            String[] contents = request.getStringArray("contents");
            if (paths == null || contents == null || paths.length != contents.length) {
                throw new IllegalArgumentException("Invalid project sources.");
            }
            Map<String, String> sources = new LinkedHashMap<>();
            for (int i = 0; i < paths.length; i++) sources.put(paths[i], contents[i]);
            byte[] dex;
            try (InputStream platform = getAssets().open("apk-builder/platform-classes.jar")) {
                dex = JavaDex.compileActivity(sources, platform, request.getString("activity"));
            }
            if (dex.length == 0 || dex.length > MAX_DEX_BYTES) {
                throw new IllegalArgumentException("Compiled app is too large for this build profile.");
            }
            result.putByteArray("dex", dex);
        } catch (Exception | LinkageError failure) {
            String detail = failure.getMessage();
            if (detail == null) detail = failure.getClass().getSimpleName();
            result.putString("error", detail.substring(0, Math.min(detail.length(), 4096)));
        }
        new Handler(Looper.getMainLooper()).post(() -> {
            // The app may bind again before Android finishes retiring the isolated process.
            compiling = false;
            Message message = Message.obtain(null, RESULT);
            message.setData(result);
            try {
                reply.send(message);
            } catch (RemoteException ignored) {
                // The requester has already closed.
            }
        });
    }
}
