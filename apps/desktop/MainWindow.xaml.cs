using System.Collections.ObjectModel;
using System.Net.WebSockets;
using System.Text.Json;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.ApplicationModel.DataTransfer;
using Windows.Graphics;
using Windows.System;
using Windows.UI;
using WinRT.Interop;

namespace JarvisDesktop;

public sealed partial class MainWindow : Window
{
    private readonly HudWindow _hud = new();
    private SpaceWindow? _space;
    private readonly VoiceLoop _voice = new();
    private readonly CancellationTokenSource _life = new();
    private BrainSession? _session;
    private ChatLine? _stream;
    private bool _streamOpen;
    private bool _hudOn;
    private bool _draftFocused;
    private string _spoken = "";
    private string _state = "parowanie";
    private string? _cardId;
    private string? _clipboardHash;
    private int _looping;

    public ObservableCollection<ChatLine> Lines { get; } = new();

    public MainWindow()
    {
        InitializeComponent();
        var host = BrainSession.HostLabel;
        HostText.Text = host;
        GateHost.Text = host;
        VoiceText.Text = _voice.Status;
        SetupShell();
        var hwnd = WindowNative.GetWindowHandle(this);
        _voice.Attach(hwnd);
        _voice.Heard += text => { _ = SendPayload(new { type = "user_text", text }); };
        _voice.StatusChanged += status => DispatcherQueue.TryEnqueue(() => VoiceText.Text = status);
        _hud.Decision += (id, accept) => { _ = Decide(id, accept); };
        Closed += OnClosed;
        Clipboard.ContentChanged += OnClipboard;
        EnsureLoop();
        _ = WatchMachine();
    }

    private void SetupShell()
    {
        var hwnd = WindowNative.GetWindowHandle(this);
        var id = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
        var app = AppWindow.GetFromWindowId(id);
        const int width = 1280;
        const int height = 860;
        app.Resize(new SizeInt32(width, height));
        try
        {
            var area = DisplayArea.GetFromWindowId(id, DisplayAreaFallback.Nearest);
            var work = area.WorkArea;
            app.Move(new PointInt32(
                work.X + Math.Max(0, (work.Width - width) / 2),
                work.Y + Math.Max(0, (work.Height - height) / 2)));
        }
        catch
        {
            /* okno zostaje tam, gdzie postawił je system */
        }
        if (app.Presenter is OverlappedPresenter presenter)
        {
            presenter.PreferredMinimumWidth = 880;
            presenter.PreferredMinimumHeight = 620;
        }
        app.Title = "Jarvis";
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        var bar = app.TitleBar;
        var clear = Color.FromArgb(0, 0, 0, 0);
        bar.BackgroundColor = clear;
        bar.InactiveBackgroundColor = clear;
        bar.ButtonBackgroundColor = clear;
        bar.ButtonInactiveBackgroundColor = clear;
        bar.ButtonForegroundColor = Rgb("8AA0B2");
        bar.ButtonInactiveForegroundColor = Rgb("8AA0B2");
        bar.ButtonHoverBackgroundColor = Rgb("1E3344");
        bar.ButtonHoverForegroundColor = Rgb("E7F6FF");
        bar.ButtonPressedBackgroundColor = Rgb("1E3344");
        bar.ButtonPressedForegroundColor = Rgb("7FDBFF");
        bar.PreferredHeightOption = TitleBarHeightOption.Tall;
    }

    private void EnsureLoop()
    {
        if (Interlocked.CompareExchange(ref _looping, 1, 0) != 0) return;
        _ = RunSession();
    }

    private async Task RunSession()
    {
        var wait = TimeSpan.FromSeconds(2);
        try
        {
            while (!_life.IsCancellationRequested)
            {
                if (BrainSession.ReadToken() is null)
                {
                    ShowGate(null);
                    return;
                }

                ShowStage();
                SetState("łączenie", "Łączenie", $"Łączę z {BrainSession.HostLabel}…");
                BrainSession? session = null;
                try
                {
                    session = new BrainSession("desktop-pc");
                    session.Event += OnEvent;
                    _session = session;
                    await session.ConnectAsync(BrainSession.ApiBase, _life.Token);
                    SetState("online", "Online", $"Połączony z {BrainSession.HostLabel}");
                    wait = TimeSpan.FromSeconds(2);
                    await session.Closed.WaitAsync(_life.Token);
                    if (!_life.IsCancellationRequested && BrainSession.ReadToken() is not null)
                        SetState("błąd", "Błąd", "Połączenie przerwane. Próbuję ponownie.");
                }
                catch (OperationCanceledException) when (_life.IsCancellationRequested)
                {
                    return;
                }
                catch (Exception ex) when (IsUnauthorized(ex))
                {
                    BrainSession.Forget();
                    ShowGate("Sparowanie wygasło. Podaj kod jeszcze raz.");
                    return;
                }
                catch (Exception ex)
                {
                    SetState("błąd", "Błąd", Describe(ex));
                }
                finally
                {
                    if (ReferenceEquals(_session, session))
                        _session = null;
                    if (session is not null)
                    {
                        session.Event -= OnEvent;
                        await session.DisposeAsync();
                    }
                }

                if (_life.IsCancellationRequested || BrainSession.ReadToken() is null)
                {
                    if (BrainSession.ReadToken() is null)
                        ShowGate(null);
                    return;
                }

                try { await Task.Delay(wait, _life.Token); }
                catch (OperationCanceledException) { return; }
                wait = TimeSpan.FromSeconds(Math.Min(30, wait.TotalSeconds * 2));
            }
        }
        finally
        {
            Interlocked.Exchange(ref _looping, 0);
        }
    }

    private void ShowGate(string? message)
    {
        RunOnUi(() =>
        {
            Stage.Visibility = Visibility.Collapsed;
            Gate.Visibility = Visibility.Visible;
            if (!string.IsNullOrWhiteSpace(message))
                GateHint.Text = message;
            ApplyState("parowanie", "Parowanie", "Czekam na kod");
            _hud.ShowLine("parowanie", string.IsNullOrWhiteSpace(message) ? "podaj kod" : message);
            Code.Focus(FocusState.Programmatic);
        });
    }

    private void ShowStage()
    {
        RunOnUi(() =>
        {
            Gate.Visibility = Visibility.Collapsed;
            Stage.Visibility = Visibility.Visible;
            if (_hudOn) return;
            _hud.Activate();
            _hudOn = true;
        });
    }

    private void SetState(string key, string pill, string detail) =>
        RunOnUi(() => ApplyState(key, pill, detail));

    private void ApplyState(string key, string pill, string detail)
    {
        _state = key;
        PillText.Text = pill;
        DetailText.Text = detail;
        var accent = Rgb("7FDBFF");
        var warn = Rgb("E8A54B");
        var danger = Rgb("FF6B6B");
        var line = Rgb("1E3344");
        var muted = Rgb("8AA0B2");
        var text = Rgb("E7F6FF");
        var bg = Rgb("070B10");
        var stroke = line;
        var pillFg = muted;
        Color? pillBg = null;
        if (key is "praca" or "łączenie")
        {
            stroke = accent;
            pillFg = bg;
            pillBg = accent;
        }
        else if (key == "zgoda")
        {
            stroke = warn;
            pillFg = warn;
        }
        else if (key == "błąd")
        {
            stroke = danger;
            pillFg = danger;
        }
        else if (key == "online")
        {
            pillFg = text;
        }

        StateLine.Background = new SolidColorBrush(stroke);
        Pill.BorderBrush = new SolidColorBrush(key is "online" or "parowanie" ? line : stroke);
        Pill.Background = new SolidColorBrush(pillBg ?? Color.FromArgb(0, 0, 0, 0));
        PillText.Foreground = new SolidColorBrush(pillFg);
        var connecting = key == "łączenie";
        Busy.IsActive = connecting;
        Busy.Visibility = connecting ? Visibility.Visible : Visibility.Collapsed;
        var ready = key is "online" or "praca" or "zgoda";
        Draft.IsEnabled = ready;
        SendButton.IsEnabled = ready;
        if (ready && !_draftFocused)
        {
            _draftFocused = true;
            Draft.Focus(FocusState.Programmatic);
        }
        if (Lines.Count == 0)
        {
            EmptyHint.Visibility = Visibility.Visible;
            EmptyHint.Text = key switch
            {
                "łączenie" => $"Łączę z {BrainSession.HostLabel}…",
                "błąd" => detail,
                _ => "Napisz tak, jakbyś mówił. Ten komputer jest przy mózgu.",
            };
        }
    }

    private async void Pair(object sender, RoutedEventArgs e) => await PairCore();

    private async void CodeKey(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Enter) return;
        e.Handled = true;
        await PairCore();
    }

    private async Task PairCore()
    {
        var code = Code.Text.Trim();
        if (code.Length == 0)
        {
            GateHint.Text = "Wpisz kod parowania.";
            return;
        }
        PairButton.IsEnabled = false;
        GateHint.Text = "Paruję…";
        try
        {
            await BrainSession.PairAsync(BrainSession.ApiBase, code);
            RunOnUi(() =>
            {
                Code.Text = "";
                GateHint.Text = "";
            });
            EnsureLoop();
        }
        catch (Exception ex)
        {
            var message = ex.Message;
            RunOnUi(() => GateHint.Text = message);
        }
        finally
        {
            RunOnUi(() => PairButton.IsEnabled = true);
        }
    }

    private void OpenSpace(object sender, RoutedEventArgs e)
    {
        _space ??= new SpaceWindow();
        _space.Activate();
    }

    private async void ForgetClick(object sender, RoutedEventArgs e)
    {
        BrainSession.Forget();
        var session = _session;
        _session = null;
        Lines.Clear();
        FinishStream();
        _cardId = null;
        ConsentPanel.Visibility = Visibility.Collapsed;
        _hud.ClearConsent();
        EmptyHint.Visibility = Visibility.Visible;
        EmptyHint.Text = "Napisz tak, jakbyś mówił. Ten komputer jest przy mózgu.";
        ShowGate("Podaj kod parowania jeszcze raz.");
        if (session is not null)
            await session.DisposeAsync();
    }

    private void OnEvent(JsonElement ev)
    {
        var type = Str(ev, "type");
        if (type == "board")
        {
            var raw = ev.GetRawText();
            DispatcherQueue.TryEnqueue(() => ApplyBoard(raw));
            return;
        }
        if (type == "hud_state")
        {
            var alert = Str(ev, "alert");
            var mission = Str(ev, "mission");
            DispatcherQueue.TryEnqueue(() => _hud.ApplyRemote(alert, mission));
            return;
        }
        var text = Str(ev, "text");
        var id = Str(ev, "id");
        var summary = Str(ev, "summary");
        var body = Str(ev, "body");
        var command = Str(ev, "command");
        var name = Str(ev, "name");
        var device = Str(ev, "device_id");
        var title = Str(ev, "title");
        string? widget = ev.TryGetProperty("widget", out var widgetEl) ? widgetEl.GetRawText() : null;
        DispatcherQueue.TryEnqueue(() => ApplyEvent(type, text, id, summary, body, command, name, device, title, widget));
    }

    private void ApplyEvent(string type, string text, string id, string summary, string body, string command, string name, string device, string title, string? widget)
    {
        switch (type)
        {
            case "token":
                Stream(text);
                foreach (var _ in _voice.Speak(text)) { }
                ApplyState("praca", "Praca", "Odpowiada");
                break;
            case "done":
                var last = _stream?.Text ?? "";
                FinishStream();
                if (_cardId is null && _state != "błąd")
                    ApplyState("online", "Online", $"Połączony z {BrainSession.HostLabel}");
                _hud.ShowLine("online", last);
                break;
            case "quiet":
                FinishStream();
                if (!string.IsNullOrWhiteSpace(text))
                    AddLine("jarvis", text);
                _hud.ShowLine("cicho", text);
                break;
            case "digest":
                FinishStream();
                var notice = string.IsNullOrWhiteSpace(body) ? title : $"{title}\n{body}";
                if (!string.IsNullOrWhiteSpace(notice))
                    AddLine("jarvis", notice);
                break;
            case "widget" when widget is not null:
            {
                using var doc = JsonDocument.Parse(widget);
                ShowWidget(doc.RootElement);
                break;
            }
            case "confirm_request":
                _cardId = id;
                ConsentSummary.Text = summary;
                ConsentBody.Text = body;
                ConsentPanel.Visibility = Visibility.Visible;
                _hud.ShowConsent(id, summary, body);
                ApplyState("zgoda", "Zgoda", summary);
                break;
            case "run":
                _ = Task.Run(async () =>
                {
                    var output = Machine.Run(command);
                    await SendPayload(new { type = "tool_result", id, output });
                });
                break;
            case "desktop_mode":
                ModeText.Text = string.IsNullOrWhiteSpace(name) ? "—" : name;
                Machine.ApplyMode(name);
                break;
            case "capture":
                _ = ReplyCapture(id);
                break;
            case "clipboard_set":
                AcceptClipboard(text);
                break;
            case "apply_patch":
                _ = Task.Run(async () =>
                {
                    var output = RepoPatch.Apply(body);
                    await SendPayload(new { type = "tool_result", id, output });
                });
                break;
            case "move_presence":
                var here = device == "desktop-pc";
                _hud.ShowLine(here ? "tutaj" : "przeniesiony", here ? "Jestem na tym komputerze." : "Przeszedłem na inne urządzenie.");
                break;
        }
    }

    private void ApplyBoard(string raw)
    {
        using var doc = JsonDocument.Parse(raw);
        var root = doc.RootElement;
        CoreText.Text = Nested(root, "scene", "text");
        if (string.IsNullOrWhiteSpace(CoreText.Text))
            CoreText.Text = "Czekam na mapę siebie.";
        var agents = LinesOf(root, "agents", item => $"{Str(item, "label")}  {Str(item, "status")}  {Str(item, "detail")}");
        AgentsText.Text = string.IsNullOrWhiteSpace(agents) ? "Brak kart." : agents;
        var feed = LinesOf(root, "feed", item => $"{Str(item, "kind")}: {Str(item, "text")}");
        FeedText.Text = string.IsNullOrWhiteSpace(feed) ? "Pusto." : feed;
        var jobs = LinesOf(root, "jobs", item => $"{Str(item, "status")}  {Str(item, "kind")}");
        TasksText.Text = string.IsNullOrWhiteSpace(jobs) ? "Zero zadań." : jobs;
        var timeline = LinesOf(root, "schedules", item => $"{Str(item, "at")}  {Str(item, "name")}");
        var evolution = LinesOf(root, "evolution", item => $"{Str(item, "status")}  {Str(item, "summary")}");
        TimelineText.Text = string.Join("\n", new[] { timeline, evolution }.Where(line => !string.IsNullOrWhiteSpace(line)));
        if (string.IsNullOrWhiteSpace(TimelineText.Text))
            TimelineText.Text = "Brak harmonogramu.";
        if (root.TryGetProperty("memory", out var memory))
        {
            var facts = Num(memory, "facts");
            var chunks = Num(memory, "chunks");
            MemoryText.Text = facts == 0 && chunks == 0
                ? "Zero notatek i faktów. Pamięć leksykalna."
                : $"fakty {facts} · fragmenty {chunks}\nkrawędzie {Num(memory, "edges")} · wiadomości {Num(memory, "messages")}\nPamięć leksykalna.";
        }
        LlmText.Text = LinesOf(root, "providers", item =>
        {
            var name = Str(item, "name");
            var set = item.TryGetProperty("set", out var flag) && flag.ValueKind == JsonValueKind.True;
            var paused = root.TryGetProperty("recent_429", out var recent)
                && recent.ValueKind == JsonValueKind.Array
                && recent.EnumerateArray().Any(entry => entry.GetString()?.Contains(name, StringComparison.OrdinalIgnoreCase) == true);
            var state = paused ? "przerwa 429" : set ? "klucz jest" : "brak klucza";
            return $"{name}  {state}";
        });
        if (string.IsNullOrWhiteSpace(LlmText.Text))
            LlmText.Text = "—";
        if (root.TryGetProperty("host", out var host) && host.TryGetProperty("free_mb", out var free))
            RamText.Text = $"{free} MB wolne";
    }

    private static string Nested(JsonElement root, string obj, string field)
    {
        if (!root.TryGetProperty(obj, out var child) || child.ValueKind != JsonValueKind.Object)
            return "";
        return Str(child, field);
    }

    private static string LinesOf(JsonElement root, string name, Func<JsonElement, string> line)
    {
        if (!root.TryGetProperty(name, out var list) || list.ValueKind != JsonValueKind.Array)
            return "";
        return string.Join("\n", list.EnumerateArray().Select(line).Where(item => !string.IsNullOrWhiteSpace(item)));
    }

    private static long Num(JsonElement obj, string name)
    {
        return obj.TryGetProperty(name, out var value) && value.TryGetInt64(out var number) ? number : 0;
    }

    private void ShowWidget(JsonElement widget)
    {
        var kind = widget.TryGetProperty("type", out var typeEl) ? typeEl.GetString() ?? "notatka" : "notatka";
        var title = widget.TryGetProperty("title", out var titleEl) && titleEl.ValueKind == JsonValueKind.String
            ? titleEl.GetString() ?? kind
            : kind;
        var body = widget.TryGetProperty("body", out var bodyEl) && bodyEl.ValueKind == JsonValueKind.String
            ? bodyEl.GetString() ?? ""
            : "";
        double? lat = widget.TryGetProperty("lat", out var latEl) && latEl.TryGetDouble(out var latValue) ? latValue : null;
        double? lon = widget.TryGetProperty("lon", out var lonEl) && lonEl.TryGetDouble(out var lonValue) ? lonValue : null;
        if (widget.TryGetProperty("center", out var center) && center.ValueKind == JsonValueKind.Array && center.GetArrayLength() >= 2
            && center[0].TryGetDouble(out var centerLon) && center[1].TryGetDouble(out var centerLat))
        {
            lon ??= centerLon;
            lat ??= centerLat;
        }
        if (lat is not null && lon is not null)
        {
            var point = $"{lat:0.0000}, {lon:0.0000}";
            body = string.IsNullOrWhiteSpace(body) ? point : $"{body}\n{point}";
        }
        WidgetTitle.Text = string.IsNullOrWhiteSpace(title) ? kind : title;
        WidgetBody.Text = body;
        WidgetCard.Visibility = Visibility.Visible;
    }

    private ChatLine AddLine(string role, string text)
    {
        var line = new ChatLine { Role = role, Text = text };
        Lines.Add(line);
        EmptyHint.Visibility = Visibility.Collapsed;
        Stick();
        return line;
    }

    private void Stream(string piece)
    {
        if (string.IsNullOrWhiteSpace(piece)) return;
        var chunk = piece.Trim();
        _spoken = string.IsNullOrEmpty(_spoken) ? chunk : $"{_spoken} {chunk}";
        if (!_streamOpen || _stream is null)
        {
            _stream = AddLine("jarvis", _spoken);
            _streamOpen = true;
        }
        else
        {
            _stream.Text = _spoken;
            Stick();
        }
        _hud.ShowLine("mówi", _spoken);
    }

    private void FinishStream()
    {
        _streamOpen = false;
        _spoken = "";
    }

    private void Stick()
    {
        TranscriptScroll.UpdateLayout();
        TranscriptScroll.ChangeView(null, TranscriptScroll.ExtentHeight, null, true);
    }

    private async void DraftKey(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Enter) return;
        e.Handled = true;
        await Submit();
    }

    private async void SendClick(object sender, RoutedEventArgs e) => await Submit();

    private async Task Submit()
    {
        var text = Draft.Text.Trim();
        if (text.Length == 0) return;
        if (_session is null)
        {
            ApplyState("błąd", "Błąd", "Jeszcze nie ma połączenia.");
            return;
        }
        Draft.Text = "";
        FinishStream();
        AddLine("ja", text);
        if (_cardId is { } card && text.Equals("tak", StringComparison.OrdinalIgnoreCase))
        {
            await Decide(card, true);
            return;
        }
        if (_cardId is { } deny && text.Equals("nie", StringComparison.OrdinalIgnoreCase))
        {
            await Decide(deny, false);
            return;
        }
        ApplyState("praca", "Praca", "Wysyłam");
        await SendPayload(new { type = "user_text", text });
    }

    private void AcceptClick(object sender, RoutedEventArgs e)
    {
        if (_cardId is { } id)
            _ = Decide(id, true);
    }

    private void RejectClick(object sender, RoutedEventArgs e)
    {
        if (_cardId is { } id)
            _ = Decide(id, false);
    }

    private async Task Decide(string id, bool accept)
    {
        if (string.IsNullOrEmpty(id) || _cardId != id) return;
        _cardId = null;
        RunOnUi(() =>
        {
            ConsentPanel.Visibility = Visibility.Collapsed;
            _hud.ClearConsent();
            ApplyState("praca", "Praca", "Wysyłam decyzję");
        });
        await SendPayload(new { type = "confirm", id, accept, via = "button" });
    }

    private async Task ReplyCapture(string id)
    {
        var shot = await Machine.CapturePngAsync() ?? "brak kadru";
        await SendPayload(new { type = "tool_result", id, output = shot });
    }

    private async Task WatchMachine()
    {
        while (!_life.IsCancellationRequested)
        {
            try
            {
                var (appName, title) = Machine.Foreground();
                var idle = Machine.Idle();
                var snap = Machine.Telemetry();
                RunOnUi(() =>
                {
                    var window = string.IsNullOrWhiteSpace(title) ? appName : $"{appName}  {title}";
                    FocusText.Text = string.IsNullOrWhiteSpace(window) ? "—" : window;
                    CpuText.Text = float.IsNaN(snap.cpu) ? "—" : $"{snap.cpu:0}%";
                    BatteryText.Text = $"{snap.battery}%";
                    NetText.Text = string.IsNullOrWhiteSpace(snap.network) ? "—" : snap.network;
                    _hud.ShowVitals(snap.cpu, snap.ram, snap.battery, snap.network);
                });
                if (_session is not null)
                {
                    var looking = appName.Equals("JarvisDesktop", StringComparison.OrdinalIgnoreCase);
                    await SendPayload(new
                    {
                        type = "presence",
                        speech = false,
                        idle = !looking && idle >= TimeSpan.FromSeconds(45),
                        away = !looking && idle >= TimeSpan.FromMinutes(3),
                        app = appName,
                        window = title,
                        telemetry = snap,
                    });
                }
            }
            catch
            {
                /* telemetria nie może zatrzymać czatu */
            }
            try { await Task.Delay(TimeSpan.FromSeconds(15), _life.Token); }
            catch (OperationCanceledException) { return; }
        }
    }

    private void OnClipboard(object? sender, object args) => _ = PushClipboard();

    private async Task PushClipboard()
    {
        if (_session is null) return;
        try
        {
            var view = Clipboard.GetContent();
            if (!view.Contains(StandardDataFormats.Text)) return;
            var text = await view.GetTextAsync();
            var hash = Hash(text);
            if (hash == _clipboardHash) return;
            _clipboardHash = hash;
            await SendPayload(new { type = "clipboard", text });
        }
        catch
        {
            /* schowek bywa zajęty */
        }
    }

    private void AcceptClipboard(string text)
    {
        try
        {
            _clipboardHash = Hash(text);
            var pack = new DataPackage();
            pack.SetText(text);
            Clipboard.SetContent(pack);
        }
        catch
        {
            /* schowek bywa zajęty */
        }
    }

    private async Task SendPayload(object payload)
    {
        var session = _session;
        if (session is null) return;
        try { await session.SendAsync(payload); }
        catch
        {
            /* pętla połączenia wznawia sesję */
        }
    }

    private void OnClosed(object sender, WindowEventArgs args)
    {
        _life.Cancel();
        Clipboard.ContentChanged -= OnClipboard;
        var session = _session;
        _session = null;
        if (session is not null)
            _ = session.DisposeAsync().AsTask();
    }

    private void RunOnUi(Action action)
    {
        if (DispatcherQueue.HasThreadAccess)
            action();
        else
            DispatcherQueue.TryEnqueue(() => action());
    }

    private static string Str(JsonElement ev, string name) =>
        ev.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String
            ? value.GetString() ?? ""
            : "";

    private static bool IsUnauthorized(Exception ex)
    {
        for (var current = ex; current is not null; current = current.InnerException)
        {
            if (current is WebSocketException && current.Message.Contains("401", StringComparison.Ordinal))
                return true;
        }
        return false;
    }

    private static string Describe(Exception ex)
    {
        if (ex is HttpRequestException or WebSocketException or IOException or TimeoutException)
            return $"Nie mogę dojść do {BrainSession.HostLabel}. Próbuję ponownie.";
        return string.IsNullOrWhiteSpace(ex.Message)
            ? "Połączenie przerwane. Próbuję ponownie."
            : ex.Message;
    }

    private static string Hash(string text) =>
        Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(text)));

    private static Color Rgb(string hex)
    {
        var value = Convert.ToUInt32(hex, 16);
        return Color.FromArgb(255, (byte)(value >> 16), (byte)(value >> 8), (byte)value);
    }
}
