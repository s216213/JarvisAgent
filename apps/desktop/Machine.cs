using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json;

namespace JarvisDesktop;

public static class Machine
{
    public static object Telemetry()
    {
        var battery = Windows.System.Power.PowerManager.RemainingChargePercent;
        return new
        {
            cpu = Cpu(),
            battery,
            network = Windows.Networking.Connectivity.NetworkInformation.GetInternetConnectionProfile()?.ProfileName ?? "",
        };
    }

    private static float Cpu()
    {
        try
        {
            using var counter = new PerformanceCounter("Processor", "% Processor Time", "_Total");
            counter.NextValue();
            Thread.Sleep(80);
            return counter.NextValue();
        }
        catch
        {
            return 0;
        }
    }

    public static (string App, string Title) Foreground()
    {
        var hwnd = GetForegroundWindow();
        var title = new char[256];
        _ = GetWindowText(hwnd, title, title.Length);
        GetWindowThreadProcessId(hwnd, out var pid);
        string name;
        try { name = Process.GetProcessById((int)pid).ProcessName; }
        catch { name = ""; }
        return (name, new string(title).Trim('\0'));
    }

    public static void ApplyMode(string name)
    {
        if (name.Contains("kod", StringComparison.OrdinalIgnoreCase))
        {
            Start("cursor");
            Start("wt");
            Start("spotify");
            _ = Task.Run(async () =>
            {
                await Task.Delay(1500);
                Tile("Cursor", 0, 0, 1100, 800);
                Tile("WindowsTerminal", 1100, 0, 700, 500);
            });
        }
        else if (name.Contains("nauk", StringComparison.OrdinalIgnoreCase))
        {
            Start("obsidian");
        }
    }

    public static void Tile(string process, int x, int y, int width, int height)
    {
        foreach (var item in Process.GetProcessesByName(process))
        {
            if (item.MainWindowHandle != 0)
                MoveWindow(item.MainWindowHandle, x, y, width, height, true);
        }
    }

    public static string Click(string label)
    {
        var hwnd = GetForegroundWindow();
        return $"Klik w „{label}” czeka na kartę zgody. Okno {hwnd}.";
    }

    public static string Run(string command)
    {
        var info = new ProcessStartInfo("powershell", $"-NoProfile -Command {command}")
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        using var process = Process.Start(info);
        if (process is null) return "brak procesu";
        if (!process.WaitForExit(30_000))
        {
            process.Kill(true);
            return "przekroczony czas";
        }
        var text = process.StandardOutput.ReadToEnd() + process.StandardError.ReadToEnd();
        return text.Length > 4000 ? text[..4000] : text;
    }

    public static async Task<string?> CapturePngAsync()
    {
        var hwnd = GetForegroundWindow();
        if (hwnd == 0) return null;
        var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
        var item = Windows.Graphics.Capture.GraphicsCaptureItem.TryCreateFromWindowId(
            new Windows.UI.WindowId(windowId.Value));
        if (item is null) return null;
        return JsonSerializer.Serialize(new { window = item.DisplayName, width = item.Size.Width, height = item.Size.Height });
    }

    private static void Start(string file)
    {
        try { Process.Start(new ProcessStartInfo(file) { UseShellExecute = true }); }
        catch { /* program może nie być zainstalowany */ }
    }

    [DllImport("user32.dll")]
    private static extern nint GetForegroundWindow();

    [DllImport("user32.dll")]
    private static extern int GetWindowText(nint hWnd, char[] text, int count);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(nint hWnd, out uint processId);

    [DllImport("user32.dll")]
    private static extern bool MoveWindow(nint hWnd, int x, int y, int width, int height, bool repaint);
}
