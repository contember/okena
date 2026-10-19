package com.okenamobile

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat

class ConnectionService : Service() {
  companion object {
    private const val CHANNEL_ID = "terminal_connection"
    private const val NOTIFICATION_ID = 1
    private const val ACTION_DISCONNECT = "com.okenamobile.DISCONNECT"

    @Volatile var running = false
      private set
    var onDisconnect: (() -> Unit)? = null
  }

  override fun onCreate() {
    super.onCreate()
    val manager = getSystemService(NotificationManager::class.java)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      manager.createNotificationChannel(
        NotificationChannel(CHANNEL_ID, "Terminal connection", NotificationManager.IMPORTANCE_LOW),
      )
    }
    val open = PendingIntent.getActivity(
      this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
    )
    val disconnect = PendingIntent.getService(
      this, 0, Intent(this, ConnectionService::class.java).setAction(ACTION_DISCONNECT),
      PendingIntent.FLAG_IMMUTABLE,
    )
    val notification = NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(R.drawable.ic_connection)
      .setContentTitle("Okena terminal session")
      .setContentText("Keeping your remote terminal connection active")
      .setContentIntent(open)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .addAction(0, "Disconnect", disconnect)
      .build()
    try {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
        startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
      } else {
        startForeground(NOTIFICATION_ID, notification)
      }
      running = true
    } catch (error: IllegalStateException) {
      Log.w("okena", "Could not promote terminal connection service", error)
      stopSelf()
    } catch (error: SecurityException) {
      Log.w("okena", "Terminal connection service permission denied", error)
      stopSelf()
    }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_DISCONNECT) {
      // Keep the process foreground until JS has closed the Rust connection.
      val disconnect = onDisconnect
      if (disconnect != null) disconnect() else stopSelf()
    }
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    running = false
    super.onDestroy()
  }

  override fun onBind(intent: Intent?): IBinder? = null
}
