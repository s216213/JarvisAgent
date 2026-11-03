package app.jarvis

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject
import kotlin.concurrent.thread

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val lineState = mutableStateOf("Tekst na pasku, dopóki nie ma głosu.")
        val brain = Brain(this) { event ->
            val type = event.optString("type")
            if (type == "clipboard_set") {
                brain.setClipboard(this, event.optString("text"))
            }
            val summary = when (type) {
                "token" -> event.optString("text")
                "space_scene" -> "Mapa siebie"
                "widget" -> event.optJSONObject("widget")?.optString("title").orEmpty()
                "hud_state" -> {
                    val mission = event.optString("mission")
                    val alert = event.optString("alert")
                    if (mission.isNotEmpty()) mission else alert
                }
                else -> ""
            }
            if (summary.isNotEmpty()) {
                val line = "$summary\n/space"
                runOnUiThread { lineState.value = line }
                HudService.show(line)
            }
        }
        val bg = Color(0xFF070B10)
        val panel = Color(0xFF101820)
        val accent = Color(0xFF7FDBFF)
        val text = Color(0xFFE7F6FF)
        val muted = Color(0xFF8AA0B2)
        val line = Color(0xFF1E3344)
        setContent {
            MaterialTheme(
                colorScheme = darkColorScheme(
                    background = bg,
                    surface = panel,
                    primary = accent,
                    onPrimary = bg,
                    onBackground = text,
                    onSurface = text,
                ),
            ) {
                var api by remember { mutableStateOf("http://10.0.2.2:8080") }
                var code by remember { mutableStateOf("") }
                val lineText by lineState
                val fieldColors = TextFieldDefaults.colors(
                    focusedContainerColor = panel,
                    unfocusedContainerColor = panel,
                    focusedTextColor = text,
                    unfocusedTextColor = text,
                    focusedIndicatorColor = accent,
                    unfocusedIndicatorColor = line,
                    cursorColor = accent,
                    focusedLabelColor = muted,
                    unfocusedLabelColor = muted,
                )
                Column(
                    Modifier.fillMaxSize().background(bg).padding(24.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    Text("Jarvis", color = accent, fontFamily = FontFamily.Monospace, fontSize = 13.sp)
                    Text("Parowanie", color = text, fontSize = 18.sp)
                    TextField(
                        api,
                        { api = it },
                        modifier = Modifier.fillMaxWidth(),
                        label = { Text("Rdzeń") },
                        colors = fieldColors,
                        shape = RoundedCornerShape(12.dp),
                    )
                    TextField(
                        code,
                        { code = it },
                        modifier = Modifier.fillMaxWidth(),
                        label = { Text("Kod") },
                        colors = fieldColors,
                        shape = RoundedCornerShape(12.dp),
                    )
                    Button(
                        onClick = {
                            thread {
                                brain.pair(api, code)
                                brain.connect()
                                runOnUiThread {
                                    lineState.value = "Sparowany.\n/space"
                                    try {
                                        startForegroundService(Intent(this, HudService::class.java))
                                    } catch (_: Exception) {
                                    }
                                }
                            }
                        },
                        colors = ButtonDefaults.buttonColors(containerColor = accent, contentColor = bg),
                        shape = RoundedCornerShape(12.dp),
                    ) { Text("Sparuj telefon") }
                    Text(lineText, color = muted, fontFamily = FontFamily.Monospace, fontSize = 13.sp)
                    Button(
                        onClick = {
                            brain.send(JSONObject().put("type", "user_text").put("text", "status"))
                        },
                        colors = ButtonDefaults.buttonColors(containerColor = panel, contentColor = text),
                        shape = RoundedCornerShape(12.dp),
                    ) { Text("Zapytaj") }
                }
            }
        }
    }
}
