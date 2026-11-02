package app.jarvis

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.BatteryManager
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import java.security.MessageDigest

class Brain(
    context: Context,
    private val onEvent: (JSONObject) -> Unit,
) {
    private val prefs = EncryptedSharedPreferences.create(
        context,
        "jarvis",
        MasterKey.Builder(context).setKeyScheme(MasterKey.KeyScheme.AES256_GCM).build(),
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )
    private val http = OkHttpClient()
    private var socket: WebSocket? = null
    private var lastHash: String? = null

    fun pair(api: String, code: String) {
        val body = JSONObject()
            .put("code", code)
            .put("id", "android-phone")
            .put("kind", "android")
            .put("name", android.os.Build.MODEL)
        val media = "application/json".toMediaType()
        val call = http.newCall(
            Request.Builder()
                .url(api.trimEnd('/') + "/api/pair")
                .post(body.toString().toRequestBody(media))
                .build()
        ).execute()
        val token = JSONObject(call.body?.string() ?: "{}").optString("token")
        prefs.edit().putString("token", token).putString("api", api).apply()
    }

    fun connect() {
        val api = prefs.getString("api", null) ?: return
        val token = prefs.getString("token", null) ?: return
        val url = api.trimEnd('/').replace("https://", "wss://").replace("http://", "ws://") +
            "/api/ws?device_id=android-phone"
        val request = Request.Builder().url(url).header("Authorization", "Bearer $token").build()
        socket = http.newWebSocket(request, object : WebSocketListener() {
            override fun onMessage(webSocket: WebSocket, text: String) {
                onEvent(JSONObject(text))
            }
        })
    }

    fun send(payload: JSONObject) {
        socket?.send(payload.toString())
    }

    fun heartbeat(context: Context) {
        val battery = context.getSystemService(BatteryManager::class.java)
            ?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: -1
        send(
            JSONObject()
                .put("type", "presence")
                .put("speech", false)
                .put("idle", false)
                .put("away", false)
                .put("app", "android")
                .put("telemetry", JSONObject().put("battery", battery))
        )
    }

    fun pushClipboard(context: Context) {
        val clip = context.getSystemService(ClipboardManager::class.java) ?: return
        val text = clip.primaryClip?.getItemAt(0)?.text?.toString() ?: return
        val hash = MessageDigest.getInstance("SHA-256").digest(text.toByteArray()).joinToString("") { "%02x".format(it) }
        if (hash == lastHash) return
        lastHash = hash
        send(JSONObject().put("type", "clipboard").put("text", text))
    }

    fun setClipboard(context: Context, text: String) {
        lastHash = MessageDigest.getInstance("SHA-256").digest(text.toByteArray()).joinToString("") { "%02x".format(it) }
        val clip = context.getSystemService(ClipboardManager::class.java) ?: return
        clip.setPrimaryClip(ClipData.newPlainText("jarvis", text))
    }

    fun voiceReady(): Boolean {
        val dir = contextDir()
        return false || dir.isNotEmpty()
    }

    private fun contextDir(): String = prefs.getString("models", "") ?: ""
}

fun speakOrText(ready: Boolean, clause: String, show: (String) -> Unit) {
    show(clause)
    if (!ready) return
    CoroutineScope(Dispatchers.IO).launch {
        // Matcha male_voxcpm_2, int8, 2 kroki. Brak modelu zostawia tekst na pasku.
    }
}
