package app.jarvis

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import org.json.JSONObject
import kotlin.concurrent.thread

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val brain = Brain(this) { event ->
            val type = event.optString("type")
            if (type == "clipboard_set") {
                brain.setClipboard(this, event.optString("text"))
            }
        }
        setContent {
            MaterialTheme {
                var api by remember { mutableStateOf("http://10.0.2.2:8080") }
                var code by remember { mutableStateOf("") }
                var line by remember { mutableStateOf("Tekst na pasku, dopóki nie ma Matchy.") }
                Column(Modifier.fillMaxSize().padding(24.dp)) {
                    Text("Jarvis", color = Color(0xFF7FDBFF))
                    TextField(api, { api = it }, label = { Text("Rdzeń") })
                    TextField(code, { code = it }, label = { Text("Kod") })
                    Button(onClick = {
                        thread {
                            brain.pair(api, code)
                            brain.connect()
                            line = "Sparowany."
                        }
                    }) { Text("Sparuj telefon") }
                    Text(line, color = Color(0xFFE7F6FF))
                    Button(onClick = {
                        brain.send(JSONObject().put("type", "user_text").put("text", "status"))
                    }) { Text("Zapytaj") }
                }
            }
        }
    }
}
