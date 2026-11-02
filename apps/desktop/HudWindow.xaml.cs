using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using WinRT.Interop;

namespace JarvisDesktop;

public sealed partial class HudWindow : Window
{
    private string? _cardId;
    public event Action<string, bool>? Decision;

    public HudWindow()
    {
        InitializeComponent();
        var hwnd = WindowNative.GetWindowHandle(this);
        var id = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
        var app = AppWindow.GetFromWindowId(id);
        app.SetPresenter(AppWindowPresenterKind.CompactOverlay);
        ExtendsContentIntoTitleBar = true;
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

    private void Accept(object sender, RoutedEventArgs e) => CloseCard(true);
    private void Reject(object sender, RoutedEventArgs e) => CloseCard(false);

    private void CloseCard(bool accept)
    {
        Consent.Visibility = Visibility.Collapsed;
        if (_cardId is { } id)
            Decision?.Invoke(id, accept);
        _cardId = null;
    }
}
