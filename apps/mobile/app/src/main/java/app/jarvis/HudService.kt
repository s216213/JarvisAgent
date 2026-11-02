package app.jarvis

import android.app.Notification
import android.app.Service
import android.content.Intent
import android.os.IBinder
import android.view.Gravity
import android.view.WindowManager
import android.widget.TextView

class HudService : Service() {
    private var label: TextView? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notice = Notification.Builder(this, "jarvis")
            .setContentTitle("Jarvis")
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .build()
        startForeground(1, notice)
        if (label == null && android.provider.Settings.canDrawOverlays(this)) {
            label = TextView(this).apply {
                text = "cisza"
                setTextColor(0xFFE7F6FF.toInt())
                setBackgroundColor(0xDD101820.toInt())
                textSize = 18f
            }
            val type = WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
            val params = WindowManager.LayoutParams(
                WindowManager.LayoutParams.WRAP_CONTENT,
                WindowManager.LayoutParams.WRAP_CONTENT,
                type,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE,
                android.graphics.PixelFormat.TRANSLUCENT,
            )
            params.gravity = Gravity.TOP
            getSystemService(WindowManager::class.java).addView(label, params)
        }
        return START_STICKY
    }
}
