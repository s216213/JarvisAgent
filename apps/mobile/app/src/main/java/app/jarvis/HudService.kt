package app.jarvis

import android.app.Notification
import android.app.Service
import android.content.Intent
import android.os.IBinder
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.widget.LinearLayout
import android.widget.TextView

class HudService : Service() {
    private var label: TextView? = null

    override fun onCreate() {
        super.onCreate()
        current = this
    }

    override fun onDestroy() {
        if (current === this) current = null
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notice = Notification.Builder(this, "jarvis")
            .setContentTitle("Jarvis")
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .build()
        startForeground(1, notice)
        if (label == null && android.provider.Settings.canDrawOverlays(this)) {
            val density = resources.displayMetrics.density
            val pad = (16 * density).toInt()
            val bar = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                setBackgroundColor(0xDB101820.toInt())
                setPadding(0, pad, pad, pad)
                gravity = Gravity.CENTER_VERTICAL
            }
            val accent = View(this).apply { setBackgroundColor(0xFF7FDBFF.toInt()) }
            bar.addView(
                accent,
                LinearLayout.LayoutParams((2 * density).toInt().coerceAtLeast(2), (18 * density).toInt()),
            )
            label = TextView(this).apply {
                text = "cisza"
                setTextColor(0xFFE7F6FF.toInt())
                textSize = 18f
                setPadding(pad, 0, 0, 0)
            }
            bar.addView(label)
            val type = WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
            val params = WindowManager.LayoutParams(
                WindowManager.LayoutParams.WRAP_CONTENT,
                WindowManager.LayoutParams.WRAP_CONTENT,
                type,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE,
                android.graphics.PixelFormat.TRANSLUCENT,
            )
            params.gravity = Gravity.TOP
            getSystemService(WindowManager::class.java).addView(bar, params)
        }
        return START_STICKY
    }

    companion object {
        private var current: HudService? = null

        fun show(text: String) {
            val view = current?.label ?: return
            view.post { view.text = text }
        }
    }
}
