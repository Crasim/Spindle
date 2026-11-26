package org.spindleapp.spindle;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;
import android.os.PowerManager;

public final class SpindleBackgroundService extends Service {
    private static final String CHANNEL = "background_work";
    private static final String REPLIES = "completed_replies";
    private static final String STOP = "org.spindleapp.spindle.STOP_BACKGROUND";
    private static final int NOTIFICATION = 781;
    private static final long WORK_WAKE_LIMIT_MS = 15 * 60 * 1000L;

    // All three fields are updated on Android's main thread.
    private static volatile SpindleBackgroundService running;
    private static boolean visible;
    private static boolean busy;
    private PowerManager.WakeLock wakeLock;
    private boolean sleepPreventedForWork;

    static boolean enabled(Context context) {
        return context.getSharedPreferences("background_work", MODE_PRIVATE).getBoolean("enabled", false);
    }

    static void setEnabled(Context context, boolean enabled) {
        context.getSharedPreferences("background_work", MODE_PRIVATE).edit()
                .putBoolean("enabled", enabled).apply();
    }

    static boolean active() {
        return running != null;
    }

    static void setVisible(boolean value) {
        visible = value;
        publishState();
    }

    static void setBusy(boolean value) {
        busy = value;
        if (running != null) running.updateWakeLock();
    }

    private static void publishState() {
        SpindleActivity.nativeBackgroundState(visible, active());
        if (running != null) running.updateWakeLock();
    }

    private void updateWakeLock() {
        if (busy && !visible) {
            if (!sleepPreventedForWork) {
                PowerManager power = getSystemService(PowerManager.class);
                wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Spindle:ActiveWork");
                wakeLock.setReferenceCounted(false);
                wakeLock.acquire(WORK_WAKE_LIMIT_MS);
                sleepPreventedForWork = true;
            }
        } else {
            releaseWakeLock();
        }
    }

    private void releaseWakeLock() {
        if (wakeLock != null && wakeLock.isHeld()) wakeLock.release();
        wakeLock = null;
        sleepPreventedForWork = false;
    }

    private Notification notification() {
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel(
                CHANNEL, "Spindle background work", NotificationManager.IMPORTANCE_LOW));
        Intent openApp = new Intent(this, SpindleActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        PendingIntent open = PendingIntent.getActivity(this, 0, openApp, PendingIntent.FLAG_IMMUTABLE);
        Intent stopService = new Intent(this, SpindleBackgroundService.class).setAction(STOP);
        PendingIntent stop = PendingIntent.getService(this, 0, stopService, PendingIntent.FLAG_IMMUTABLE);
        Notification.Builder notification = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_background_work)
                .setContentTitle("Spindle background mode")
                .setContentText("Sync and existing tasks can continue")
                .setContentIntent(open)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setVisibility(Notification.VISIBILITY_PRIVATE)
                .addAction(new Notification.Action.Builder(null, "Turn off", stop).build());
        if (Build.VERSION.SDK_INT >= 31) {
            notification.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE);
        }
        return notification.build();
    }

    static boolean repliesEnabled(Context context) {
        NotificationManager manager = context.getSystemService(NotificationManager.class);
        NotificationChannel replies = manager.getNotificationChannel(REPLIES);
        return manager.areNotificationsEnabled()
                && (replies == null || replies.getImportance() != NotificationManager.IMPORTANCE_NONE);
    }

    static void notifyReply(long chat) {
        SpindleBackgroundService service = running;
        if (service == null || visible || !enabled(service)) return;
        NotificationManager manager = service.getSystemService(NotificationManager.class);
        if (!repliesEnabled(service)) return;
        manager.createNotificationChannel(new NotificationChannel(
                REPLIES, "Completed replies", NotificationManager.IMPORTANCE_DEFAULT));
        Intent intent = new Intent(service, SpindleActivity.class)
                .setData(android.net.Uri.parse("spindle://reply/" + chat))
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        PendingIntent open = PendingIntent.getActivity(service, 0, intent, PendingIntent.FLAG_IMMUTABLE);
        Notification notification = new Notification.Builder(service, REPLIES)
                .setSmallIcon(R.drawable.ic_background_work)
                .setContentTitle("Spindle reply ready")
                .setContentText("Tap to open the conversation")
                .setContentIntent(open)
                .setAutoCancel(true)
                .setVisibility(Notification.VISIBILITY_PRIVATE)
                .build();
        manager.notify("reply-" + chat, 782, notification);
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && STOP.equals(intent.getAction())) {
            setEnabled(this, false);
            stopSelf();
            return START_NOT_STICKY;
        }
        try {
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(NOTIFICATION, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC);
            } else {
                startForeground(NOTIFICATION, notification());
            }
            if (enabled(this)) {
                running = this;
                publishState();
            } else {
                stopSelf();
            }
        } catch (RuntimeException error) {
            android.util.Log.w("Spindle", "Background service could not start", error);
            stopSelf();
        }
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        if (running == this) {
            running = null;
            publishState();
        }
        releaseWakeLock();
        super.onDestroy();
    }

    // Android 15+ may end a data sync service after its allowed background time.
    public void onTimeout(int startId, int type) {
        stopSelf();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
