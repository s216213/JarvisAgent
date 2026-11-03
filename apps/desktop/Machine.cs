using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json.Serialization;
using System.Windows.Automation;
using Windows.Graphics.Imaging;
using Windows.Storage.Streams;

namespace JarvisDesktop;

public readonly record struct Pulse(
    [property: JsonPropertyName("cpu")] float cpu,
    [property: JsonPropertyName("ram")] float ram,
    [property: JsonPropertyName("battery")] int battery,
    [property: JsonPropertyName("network")] string network);

public static class Machine
{
    public static Pulse Telemetry()
    {
        var battery = Windows.System.Power.PowerManager.RemainingChargePercent;
        var network = Windows.Networking.Connectivity.NetworkInformation.GetInternetConnectionProfile()?.ProfileName ?? "";
        return new Pulse(Cpu(), Ram(), battery, network);
    }

    public static TimeSpan Idle()
    {
        var info = new LASTINPUTINFO { cbSize = (uint)Marshal.SizeOf<LASTINPUTINFO>() };
        if (!GetLastInputInfo(ref info)) return TimeSpan.Zero;
        var idle = unchecked((uint)Environment.TickCount - info.dwTime);
        return TimeSpan.FromMilliseconds(idle);
    }

    private static float Ram()
    {
        try
        {
            var status = new MEMORYSTATUSEX { dwLength = (uint)Marshal.SizeOf<MEMORYSTATUSEX>() };
            return GlobalMemoryStatusEx(ref status) ? status.dwMemoryLoad : float.NaN;
        }
        catch
        {
            return float.NaN;
        }
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
            return float.NaN;
        }
    }

    public static (string App, string Title) Foreground()
    {
        var hwnd = GetForegroundWindow();
        var title = new char[512];
        var length = GetWindowText(hwnd, title, title.Length);
        GetWindowThreadProcessId(hwnd, out var pid);
        string name;
        try { name = Process.GetProcessById((int)pid).ProcessName; }
        catch { name = ""; }
        return (name, length > 0 ? new string(title, 0, length) : "");
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
        if (hwnd == 0)
            return "Nie ma okna na wierzchu.";
        try
        {
            var root = AutomationElement.FromHandle(hwnd);
            if (root is null)
                return "Nie czytam okna na wierzchu.";
            var found = root.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, label));
            if (found is null)
                return $"Nie widzę „{label}” w oknie na wierzchu.";
            if (found.TryGetCurrentPattern(InvokePattern.Pattern, out var pattern) && pattern is InvokePattern invoke)
            {
                invoke.Invoke();
                return $"Kliknąłem „{label}”.";
            }
            return $"„{label}” nie da się wcisnąć.";
        }
        catch (Exception ex)
        {
            return string.IsNullOrWhiteSpace(ex.Message) ? "Klik nie doszedł." : ex.Message;
        }
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
        if (item is null) return "Nie mogę objąć okna na wierzchu.";
        var width = Math.Max(1, (int)item.Size.Width);
        var height = Math.Max(1, (int)item.Size.Height);
        var pixels = CapturePixels(hwnd, width, height);
        if (pixels is null) return "Nie mam klatki PNG.";
        var png = await EncodePngAsync(pixels, width, height);
        return "data:image/png;base64," + Convert.ToBase64String(png);
    }

    private static byte[]? CapturePixels(nint hwnd, int width, int height)
    {
        var screen = GetDC(0);
        var memory = CreateCompatibleDC(screen);
        var bitmap = CreateCompatibleBitmap(screen, width, height);
        var previous = SelectObject(memory, bitmap);
        var printed = PrintWindow(hwnd, memory, 2);
        if (!printed)
            printed = BitBlt(memory, 0, 0, width, height, screen, 0, 0, 0x00CC0020);
        var info = new BITMAPINFOHEADER
        {
            biSize = (uint)Marshal.SizeOf<BITMAPINFOHEADER>(),
            biWidth = width,
            biHeight = -height,
            biPlanes = 1,
            biBitCount = 32,
        };
        var pixels = new byte[width * height * 4];
        var read = GetDIBits(memory, bitmap, 0, (uint)height, pixels, ref info, 0);
        SelectObject(memory, previous);
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(0, screen);
        if (read == 0 || !printed) return null;
        for (var i = 3; i < pixels.Length; i += 4)
        {
            if (pixels[i] == 0) pixels[i] = 255;
        }
        return pixels;
    }

    private static async Task<byte[]> EncodePngAsync(byte[] pixels, int width, int height)
    {
        var stream = new InMemoryRandomAccessStream();
        var encoder = await BitmapEncoder.CreateAsync(BitmapEncoder.PngEncoderId, stream);
        encoder.SetPixelData(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied, (uint)width, (uint)height, 96, 96, pixels);
        await encoder.FlushAsync();
        var bytes = new byte[stream.Size];
        using var reader = new DataReader(stream.GetInputStreamAt(0));
        await reader.LoadAsync((uint)bytes.Length);
        reader.ReadBytes(bytes);
        return bytes;
    }

    private static void Start(string file)
    {
        try { Process.Start(new ProcessStartInfo(file) { UseShellExecute = true }); }
        catch { /* program może nie być zainstalowany */ }
    }

    [DllImport("user32.dll")]
    private static extern nint GetForegroundWindow();

    [DllImport("user32.dll")]
    private static extern nint GetDC(nint hwnd);

    [DllImport("user32.dll")]
    private static extern int ReleaseDC(nint hwnd, nint hdc);

    [DllImport("user32.dll")]
    private static extern bool PrintWindow(nint hwnd, nint hdc, uint flags);

    [DllImport("gdi32.dll")]
    private static extern nint CreateCompatibleDC(nint hdc);

    [DllImport("gdi32.dll")]
    private static extern nint CreateCompatibleBitmap(nint hdc, int width, int height);

    [DllImport("gdi32.dll")]
    private static extern nint SelectObject(nint hdc, nint obj);

    [DllImport("gdi32.dll")]
    private static extern bool DeleteObject(nint obj);

    [DllImport("gdi32.dll")]
    private static extern bool DeleteDC(nint hdc);

    [DllImport("gdi32.dll")]
    private static extern bool BitBlt(nint dest, int x, int y, int width, int height, nint src, int srcX, int srcY, uint rop);

    [DllImport("gdi32.dll")]
    private static extern int GetDIBits(nint hdc, nint bitmap, uint start, uint lines, byte[] bits, ref BITMAPINFOHEADER info, uint usage);

    [StructLayout(LayoutKind.Sequential)]
    private struct BITMAPINFOHEADER
    {
        public uint biSize;
        public int biWidth;
        public int biHeight;
        public ushort biPlanes;
        public ushort biBitCount;
        public uint biCompression;
        public uint biSizeImage;
        public int biXPelsPerMeter;
        public int biYPelsPerMeter;
        public uint biClrUsed;
        public uint biClrImportant;
    }

    [DllImport("user32.dll")]
    private static extern int GetWindowText(nint hWnd, char[] text, int count);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(nint hWnd, out uint processId);

    [DllImport("user32.dll")]
    private static extern bool MoveWindow(nint hWnd, int x, int y, int width, int height, bool repaint);

    [StructLayout(LayoutKind.Sequential)]
    private struct LASTINPUTINFO
    {
        public uint cbSize;
        public uint dwTime;
    }

    [DllImport("user32.dll")]
    private static extern bool GetLastInputInfo(ref LASTINPUTINFO info);

    [StructLayout(LayoutKind.Sequential)]
    private struct MEMORYSTATUSEX
    {
        public uint dwLength;
        public uint dwMemoryLoad;
        public ulong ullTotalPhys;
        public ulong ullAvailPhys;
        public ulong ullTotalPageFile;
        public ulong ullAvailPageFile;
        public ulong ullTotalVirtual;
        public ulong ullAvailVirtual;
        public ulong ullAvailExtendedVirtual;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX lpBuffer);
}
