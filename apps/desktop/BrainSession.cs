using System.Net;
using System.Net.WebSockets;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace JarvisDesktop;

public sealed class BrainSession : IAsyncDisposable
{
    public const string DefaultApiBase = "https://gawr-jarvis.duckdns.org";

    public static string ApiBase
    {
        get
        {
            try
            {
                var path = Path.Combine(Path.GetDirectoryName(PairTokenPath)!, "api.txt");
                if (File.Exists(path))
                {
                    var text = File.ReadAllText(path).Trim();
                    if (Uri.TryCreate(text, UriKind.Absolute, out var uri) && uri.Scheme is "https" or "http")
                        return uri.GetLeftPart(UriPartial.Authority);
                }
            }
            catch
            {
                /* zostaje adres zapasowy */
            }
            return DefaultApiBase;
        }
    }

    private readonly string _deviceId;
    private readonly SemaphoreSlim _send = new(1, 1);
    private readonly TaskCompletionSource _closed = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private ClientWebSocket? _socket;
    private int _disposed;

    public BrainSession(string deviceId) => _deviceId = deviceId;

    public event Action<JsonElement>? Event;

    public Task Closed => _closed.Task;

    public static string HostLabel => new Uri(ApiBase).Host;

    public static string PairTokenPath =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Jarvis", "token.txt");

    public static Uri WebSocketUri(string apiBase, string deviceId)
    {
        var uri = new Uri(apiBase);
        var scheme = uri.Scheme.Equals("https", StringComparison.OrdinalIgnoreCase) ? "wss" : "ws";
        return new UriBuilder(uri)
        {
            Scheme = scheme,
            Path = "/api/ws",
            Query = $"device_id={Uri.EscapeDataString(deviceId)}",
        }.Uri;
    }

    public async Task ConnectAsync(string apiBase, CancellationToken cancel)
    {
        var token = ReadToken() ?? throw new InvalidOperationException("Brak sparowania.");
        var socket = new ClientWebSocket();
        socket.Options.KeepAliveInterval = TimeSpan.FromSeconds(20);
        socket.Options.SetRequestHeader("Authorization", $"Bearer {token.Trim()}");
        _socket = socket;
        await socket.ConnectAsync(WebSocketUri(apiBase, _deviceId), cancel);
        _ = ReceiveLoop(socket, cancel);
    }

    public static async Task PairAsync(string apiBase, string code)
    {
        if (string.IsNullOrWhiteSpace(code))
            throw new InvalidOperationException("Wpisz kod parowania.");
        Directory.CreateDirectory(Path.GetDirectoryName(PairTokenPath)!);
        using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(20) };
        var body = JsonSerializer.Serialize(new
        {
            code = code.Trim(),
            id = "desktop-pc",
            kind = "desktop",
            name = Environment.MachineName,
        });
        HttpResponseMessage response;
        try
        {
            response = await http.PostAsync(
                $"{apiBase.TrimEnd('/')}/api/pair",
                new StringContent(body, Encoding.UTF8, "application/json"));
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException)
        {
            throw new InvalidOperationException("Nie mogę dojść do gawr-jarvis.duckdns.org.");
        }

        using (response)
        {
            if (response.StatusCode == HttpStatusCode.Unauthorized)
                throw new InvalidOperationException("Zły kod parowania.");
            if (!response.IsSuccessStatusCode)
                throw new InvalidOperationException("Serwer odrzucił parowanie.");
            using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
            var token = doc.RootElement.GetProperty("token").GetString() ?? "";
            if (string.IsNullOrWhiteSpace(token))
                throw new InvalidOperationException("Serwer nie oddał tokenu.");
            var protectedBytes = ProtectedData.Protect(Encoding.UTF8.GetBytes(token), null, DataProtectionScope.CurrentUser);
            await File.WriteAllBytesAsync(PairTokenPath, protectedBytes);
        }
    }

    public static void Forget()
    {
        if (File.Exists(PairTokenPath))
            File.Delete(PairTokenPath);
    }

    public static string? ReadToken()
    {
        if (!File.Exists(PairTokenPath)) return null;
        var raw = File.ReadAllBytes(PairTokenPath);
        try
        {
            var text = Encoding.UTF8.GetString(ProtectedData.Unprotect(raw, null, DataProtectionScope.CurrentUser));
            return string.IsNullOrWhiteSpace(text) ? null : text;
        }
        catch
        {
            var text = Encoding.UTF8.GetString(raw);
            return string.IsNullOrWhiteSpace(text) ? null : text;
        }
    }

    public async Task SendAsync(object payload)
    {
        var socket = _socket;
        if (socket is null || socket.State != WebSocketState.Open)
            throw new InvalidOperationException("Brak połączenia.");
        var bytes = Encoding.UTF8.GetBytes(JsonSerializer.Serialize(payload));
        await _send.WaitAsync();
        try
        {
            if (socket.State != WebSocketState.Open) return;
            await socket.SendAsync(bytes, WebSocketMessageType.Text, true, CancellationToken.None);
        }
        finally
        {
            _send.Release();
        }
    }

    public async ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) == 1) return;
        var socket = _socket;
        _socket = null;
        if (socket is not null)
        {
            try
            {
                if (socket.State == WebSocketState.Open)
                    await socket.CloseAsync(WebSocketCloseStatus.NormalClosure, "bye", CancellationToken.None);
            }
            catch
            {
                /* gniazdo i tak zamykamy */
            }
            socket.Dispose();
        }
        _closed.TrySetResult();
    }

    private async Task ReceiveLoop(ClientWebSocket socket, CancellationToken cancel)
    {
        var buffer = new byte[64 * 1024];
        using var message = new MemoryStream();
        try
        {
            while (socket.State == WebSocketState.Open && !cancel.IsCancellationRequested)
            {
                var result = await socket.ReceiveAsync(buffer, cancel);
                if (result.MessageType == WebSocketMessageType.Close) break;
                message.Write(buffer, 0, result.Count);
                if (!result.EndOfMessage) continue;
                if (result.MessageType == WebSocketMessageType.Text && message.Length > 0)
                {
                    var text = Encoding.UTF8.GetString(message.GetBuffer(), 0, (int)message.Length);
                    try
                    {
                        using var doc = JsonDocument.Parse(text);
                        Event?.Invoke(doc.RootElement.Clone());
                    }
                    catch
                    {
                        /* uszkodzona ramka nie rozłącza sesji */
                    }
                }
                message.SetLength(0);
            }
        }
        catch
        {
            /* koniec gniazda budzi pętlę połączenia */
        }
        finally
        {
            _closed.TrySetResult();
        }
    }
}
