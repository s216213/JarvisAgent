package app.jarvis

import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import org.json.JSONObject

class NoticeReader : NotificationListenerService() {
    override fun onNotificationPosted(sbn: StatusBarNotification) {
        val extra = sbn.notification.extras
        val title = extra.getString("android.title") ?: return
        val text = extra.getCharSequence("android.text")?.toString() ?: ""
        val brain = Brain(this) { }
        brain.connect()
        brain.send(
            JSONObject()
                .put("type", "digest")
                .put("app", sbn.packageName)
                .put("title", title)
                .put("body", text)
        )
    }
}
