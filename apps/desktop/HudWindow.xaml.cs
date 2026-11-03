using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Windows.Graphics;
using WinRT.Interop;

namespace JarvisDesktop;

public sealed partial class HudWindow : Window
{
    private readonly DispatcherTimer _clock;
    private string? _cardId;
    public event Action<string, bool>? Decision;

    public HudWindow()
    {
        InitializeComponent();
        var hwnd = WindowNative.GetWindowHandle(this);
        var id = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
        var app = AppWindow.GetFromWindowId(id);
        app.Resize(new SizeInt32(640, 248));
        app.SetPresenter(AppWindowPresenterKind.CompactOverlay);
        ExtendsContentIntoTitleBar = true;
        Clock.Text = DateTime.Now.ToString("HH:mm");
        _clock = new DispatcherTimer { Interval = TimeSpan.FromSeconds(1) };
        _clock.Tick += (_, _) => Clock.Text = DateTime.Now.ToString("HH:mm");
        _clock.Start();
    }

    public void ShowVitals(float cpu, float ram, int battery, string network)
    {
        var cpuText = float.IsNaN(cpu) ? "—" : $"{cpu:0}%";
        var ramText = float.IsNaN(ram) ? "—" : $"{ram:0}%";
        var net = string.IsNullOrWhiteSpace(network) ? "—" : network;
        Vitals.Text = $"CPU {cpuText}   RAM {ramText}   BAT {battery}%   {net}";
    }

    public void ApplyRemote(string alert, string mission)
    {
        Alert.Text = alert;
        Alert.Visibility = string.IsNullOrWhiteSpace(alert) ? Visibility.Collapsed : Visibility.Visible;
        Mission.Text = mission;
        Mission.Visibility = string.IsNullOrWhiteSpace(mission) ? Visibility.Collapsed : Visibility.Visible;
    }

    public void ShowLine(string status, string text)
    {
        Status.Text = status;
        Line.Text = text;
    }

    public void ShowConsent(string id, string summary, string body)
    {
        _cardId = id;
        Status.Text = "zgoda";
        Line.Text = summary;
        ConsentBody.Text = body;
        Consent.Visibility = Visibility.Visible;
    }

    public void ClearConsent()
    {
        _cardId = null;
        Consent.Visibility = Visibility.Collapsed;
    }

    private void Accept(object sender, RoutedEventArgs e) => CloseCard(true);
    private void Reject(object sender, RoutedEventArgs e) => CloseCard(false);

    private void CloseCard(bool accept)
    {
        var id = _cardId;
        _cardId = null;
        Consent.Visibility = Visibility.Collapsed;
        if (id is not null)
            Decision?.Invoke(id, accept);
    }
}
