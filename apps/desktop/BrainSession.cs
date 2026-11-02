using System.Net.WebSockets;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace JarvisDesktop;

public sealed class BrainSession : IAsyncDisposable
{
    private readonly ClientWebSocket _socket = new();
    private readonly string _deviceId;
    public event Action<JsonElement>? Event;

    public BrainSession(string deviceId) => _deviceId = deviceId;

    public static string PairTokenPath =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Jarvis", "token.txt");

    public async Task ConnectAsync(string apiBase, CancellationToken cancel)
    {
        var token = ReadToken() ?? throw new InvalidOperationException("Brak sparowania.");
        _socket.Options.SetRequestHeader("Authorization", $"Bearer {token.Trim()}");
        var uri = new Uri($"{apiBase.TrimEnd('/')}/api/ws?device_id={_deviceId}");
        await _socket.ConnectAsync(uri, cancel);
        _ = ReceiveLoop(cancel);
    }

    public static async Task PairAsync(string apiBase, string code)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(PairTokenPath)!);
        using var http = new HttpClient();
        var body = JsonSerializer.Serialize(new { code, id = "desktop-pc", kind = "desktop", name = Environment.MachineName });
        var response = await http.PostAsync($"{apiBase.TrimEnd('/')}/api/pair", new StringContent(body, Encoding.UTF8, "application/json"));
        response.EnsureSuccessStatusCode();
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        var token = doc.RootElement.GetProperty("token").GetString() ?? "";
        var bytes = Encoding.UTF8.GetBytes(token);
        var protectedBytes = ProtectedData.Protect(bytes, null, DataProtectionScope.CurrentUser);
        await File.WriteAllBytesAsync(PairTokenPath, protectedBytes);
    }

    public static string? ReadToken()
    {
        if (!File.Exists(PairTokenPath)) return null;
        var raw = File.ReadAllBytes(PairTokenPath);
        try
        {
            return Encoding.UTF8.GetString(ProtectedData.Unprotect(raw, null, DataProtectionScope.CurrentUser));
        }
        catch
        {
            return Encoding.UTF8.GetString(raw);
        }
    }

    public async Task SendAsync(object payload)
    {
        var json = JsonSerializer.Serialize(payload);
        var bytes = Encoding.UTF8.GetBytes(json);
        await _socket.SendAsync(bytes, WebSocketMessageType.Text, true, CancellationToken.None);
    }

    private async Task ReceiveLoop(CancellationToken cancel)
    {
        var buffer = new byte[1024 * 64];
        while (_socket.State == WebSocketState.Open && !cancel.IsCancellationRequested)
        {
            var result = await _socket.ReceiveAsync(buffer, cancel);
            if (result.MessageType == WebSocketMessageType.Close) break;
            var text = Encoding.UTF8.GetString(buffer, 0, result.Count);
            using var doc = JsonDocument.Parse(text);
            Event?.Invoke(doc.RootElement.Clone());
        }
    }

    public async ValueTask DisposeAsync()
    {
        if (_socket.State == WebSocketState.Open)
            await _socket.CloseAsync(WebSocketCloseStatus.NormalClosure, "bye", CancellationToken.None);
        _socket.Dispose();
    }
}
