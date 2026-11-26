package org.spindleapp.spindle.builder;

import android.app.Activity;
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
import android.os.RemoteException;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;
import java.io.File;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.file.Files;

/** Development-only app for checking the worker on real Android. */
public final class BuildProbeActivity extends Activity {
    private TextView status;
    private Button save;
    private File output;
    private boolean bound;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable deadline = () -> finishProbe("Probe timed out after 45 seconds.");
    private final Messenger reply = new Messenger(new Handler(Looper.getMainLooper(), message -> {
        if (message.what == 2 && bound) {
            String text = message.getData().getString("status");
            byte[] dex = message.getData().getByteArray("dex");
            finishProbe(text);
            if (dex != null && dex.length > 0 && dex.length <= 64 * 1024) packageFixture(dex);
        }
        return true;
    }));
    private final ServiceConnection connection = new ProbeConnection();

    private final class ProbeConnection implements ServiceConnection {
        @Override
        public void onServiceConnected(ComponentName name, IBinder service) {
            Message message = Message.obtain(null, 1);
            message.replyTo = reply;
            try {
                new Messenger(service).send(message);
            } catch (RemoteException unavailable) {
                finishProbe("Could not start the isolated compiler.");
            }
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            finishProbe("The isolated compiler process closed before responding.");
        }

        @Override
        public void onNullBinding(ComponentName name) {
            finishProbe("The isolated compiler did not provide an IPC endpoint.");
        }
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        status = new TextView(this);
        status.setTextSize(18);
        int margin = Math.round(24 * getResources().getDisplayMetrics().density);
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setPadding(margin, margin, margin, margin);
        layout.addView(status);
        save = new Button(this);
        save.setText("Save fixture APK");
        save.setEnabled(false);
        save.setOnClickListener(view -> startActivityForResult(
                new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                        .setType("application/vnd.android.package-archive")
                        .putExtra(Intent.EXTRA_TITLE, "SpindleJavaFixture.apk"), 1));
        layout.addView(save);
        setContentView(layout);
        status.setText("Testing the bundled Java compiler in an isolated Android process…");
        try (OutputStream marker = openFileOutput("build-probe-marker", MODE_PRIVATE)) {
            marker.write(1);
        } catch (IOException failure) {
            finishProbe("Could not prepare the private file check.");
            return;
        }
        bound = bindService(new Intent(this, BuildProbeService.class), connection, Context.BIND_AUTO_CREATE);
        if (bound) {
            handler.postDelayed(deadline, 45_000);
        } else {
            finishProbe("Android could not bind the isolated compiler service.");
        }
    }

    private void packageFixture(byte[] dex) {
        String report = status.getText().toString();
        status.setText(report + "\n\nSigning the fixture APK…");
        Context application = getApplicationContext();
        new Thread(() -> {
            try {
                File apk = FixtureApk.build(application, dex);
                runOnUiThread(() -> {
                    if (isDestroyed()) return;
                    output = apk;
                    status.setText(report + "\n\nThe fixture APK was signed and verified on this phone.");
                    save.setEnabled(true);
                });
            } catch (Exception | LinkageError failure) {
                runOnUiThread(() -> {
                    if (!isDestroyed()) status.setText(report + "\n\nAPK packaging failed: " + failure);
                });
            }
        }, "sign-java-fixture").start();
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != 1 || result != RESULT_OK || data == null || data.getData() == null || output == null) {
            return;
        }
        try (OutputStream destination = getContentResolver().openOutputStream(data.getData())) {
            if (destination == null) throw new IOException("Could not open selected file.");
            Files.copy(output.toPath(), destination);
            status.append("\n\nSaved fixture APK.");
        } catch (IOException failure) {
            status.append("\n\nCould not save the APK: " + failure.getMessage());
        }
    }

    private void finishProbe(String message) {
        handler.removeCallbacks(deadline);
        if (bound) {
            bound = false;
            unbindService(connection);
        }
        status.setText(message);
    }

    @Override
    protected void onDestroy() {
        finishProbe("Probe closed.");
        super.onDestroy();
    }
}
