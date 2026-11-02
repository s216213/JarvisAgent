using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;

namespace JarvisDesktop;

public sealed partial class MainWindow : Window
{
    private readonly HudWindow _hud = new();
    private readonly VoiceLoop _voice = new();
    private BrainSession? _session;
    private string _spoken = "";
    private string? _clipboardHash;

    public MainWindow()
    {
        InitializeComponent();
        VoiceStatus.Text = _voice.Status;
        _hud.Activate();
        _hud.ShowLine(_voice.Ready ? "słucham" : "tekst", _voice.Status);
        _hud.Decision += async (id, accept) =>
        {
            if (_session is not null)
                await _session.SendAsync(new { type = "confirm", id, accept, via = "button" });
        };
        _ = Connect();
        _ = Loop();
        Clipboard.ContentChanged += async (_, _) => await PushClipboard();
    }

    private async Task Connect()
    {
        if (BrainSession.ReadToken() is null) return;
        _session = new BrainSession("desktop-pc");
        _session.Event += OnEvent;
        try
        {
            await _session.ConnectAsync(Api.Text, CancellationToken.None);
            Hint.Text = "Połączony. Alt+Space otwiera paletę.";
        }
        catch (Exception ex)
        {
            Hint.Text = ex.Message;
        }
    }

    private async void Pair(object sender, RoutedEventArgs e)
    {
        try
        {
            await BrainSession.PairAsync(Api.Text, Code.Text);
            await Connect();
        }
        catch (Exception ex)
        {
            Hint.Text = ex.Message;
        }
    }

    private void OnEvent(System.Text.Json.JsonElement ev)
    {
        var type = ev.TryGetProperty("type", out var t) ? t.GetString() : "";
        DispatcherQueue.TryEnqueue(() =>
        {
            if (type == "token")
            {
                _spoken += ev.GetProperty("text").GetString() + " ";
                _hud.ShowLine("mówi", _spoken.Trim());
                foreach (var _clause in _voice.Speak(ev.GetProperty("text").GetString() ?? "")) { }
            }
            else if (type == "done")
            {
                _spoken = "";
                _hud.ShowLine("cisza", "");
            }
            else if (type == "confirm_request")
            {
                _hud.ShowConsent(
                    ev.GetProperty("id").GetString() ?? "",
                    ev.GetProperty("summary").GetString() ?? "",
                    ev.GetProperty("body").GetString() ?? "");
            }
            else if (type == "run")
            {
                var output = Machine.Run(ev.GetProperty("command").GetString() ?? "");
                _ = _session?.SendAsync(new { type = "tool_result", id = ev.GetProperty("id").GetString(), output });
            }
            else if (type == "desktop_mode")
            {
                Machine.ApplyMode(ev.GetProperty("name").GetString() ?? "");
            }
            else if (type == "capture")
            {
                _ = ReplyCapture(ev.GetProperty("id").GetString() ?? "");
            }
            else if (type == "clipboard_set")
            {
                var text = ev.GetProperty("text").GetString() ?? "";
                _clipboardHash = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(text)));
                var pack = new DataPackage();
                pack.SetText(text);
                Clipboard.SetContent(pack);
            }
        });
    }

    private async Task ReplyCapture(string id)
    {
        var shot = await Machine.CapturePngAsync() ?? "brak kadru";
        if (_session is not null)
            await _session.SendAsync(new { type = "tool_result", id, output = shot });
    }

    private async Task Loop()
    {
        while (true)
        {
            await Task.Delay(TimeSpan.FromSeconds(15));
            if (_session is null) continue;
            var (app, title) = Machine.Foreground();
            var hour = DateTime.Now.Hour;
            var suggestion = title.Contains("Cursor", StringComparison.OrdinalIgnoreCase)
                ? "Skompiluj projekt"
                : hour >= 18 ? "Otwórz notatnik codzienny" : "Pokaż status serwera";
            DispatcherQueue.TryEnqueue(() => Palette.PlaceholderText = suggestion);
            await _session.SendAsync(new
            {
                type = "presence",
                speech = false,
                idle = false,
                away = false,
                app,
                window = title,
                telemetry = Machine.Telemetry(),
            });
        }
    }

    private async void PaletteKey(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != Windows.System.VirtualKey.Enter || _session is null) return;
        var text = Palette.Text.Trim();
        if (text.Length == 0) return;
        Log.Text = text;
        Palette.Text = "";
        await _session.SendAsync(new { type = "user_text", text });
    }

    private async Task PushClipboard()
    {
        if (_session is null) return;
        var view = Clipboard.GetContent();
        if (!view.Contains(StandardDataFormats.Text)) return;
        var text = await view.GetTextAsync();
        var hash = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(text)));
        if (hash == _clipboardHash) return;
        _clipboardHash = hash;
        await _session.SendAsync(new { type = "clipboard", text });
    }
}
