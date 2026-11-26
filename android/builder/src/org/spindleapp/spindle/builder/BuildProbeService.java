package org.spindleapp.spindle.builder;

import android.app.Service;
import android.content.Intent;
import android.os.IBinder;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.Message;
import android.os.Messenger;
import android.os.Process;
import android.os.RemoteException;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;

/** One fixed compiler fixture, callable only from the private probe app. */
public final class BuildProbeService extends Service {
    private boolean started;
    private final Messenger binder = new Messenger(new Handler(Looper.getMainLooper(), message -> {
        if (started || message.what != 1 || message.replyTo == null) return true;
        started = true;
        Messenger reply = message.replyTo;
        new Thread(() -> runProbe(reply), "java-dex-probe").start();
        return true;
    }));

    @Override
    public IBinder onBind(Intent intent) {
        return binder.getBinder();
    }

    private void runProbe(Messenger reply) {
        Bundle result = new Bundle();
        try {
            if (Process.myUid() == getApplicationInfo().uid) {
                throw new IOException("Compiler worker has the main app's UID.");
            }
            try (FileInputStream marker = new FileInputStream(
                    new java.io.File(getApplicationInfo().dataDir, "files/build-probe-marker"))) {
                throw new IOException("Compiler worker could open the main app's private file.");
            } catch (java.io.FileNotFoundException denied) {
                // The caller verified that the marker exists before binding the service.
            }
            byte[] dex;
            try (InputStream platform = getAssets().open("platform-classes.jar")) {
                dex = BuildFixture.run(platform);
            }
            if (dex.length > 64 * 1024) throw new IOException("Fixture DEX exceeded the IPC limit.");
            result.putByteArray("dex", dex);
            result.putString("status", "Passed. The isolated worker compiled Java into "
                    + dex.length + " bytes of DEX and could not open the app's private file.");
        } catch (Exception | LinkageError failure) {
            result.putString("status", "Probe failed: " + failure);
        }
        Message message = Message.obtain(null, 2);
        message.setData(result);
        try {
            reply.send(message);
        } catch (RemoteException closed) {
            // The caller left the screen.
        }
    }
}
