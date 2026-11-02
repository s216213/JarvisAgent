using System.Runtime.InteropServices;

namespace JarvisDesktop;

public sealed class VoiceLoop
{
    private readonly string _modelDir;
    public bool Ready { get; }
    public string Status { get; }

    public VoiceLoop()
    {
        _modelDir = Path.Combine(AppContext.BaseDirectory, "models");
        var wake = Path.Combine(_modelDir, "hey_jarvis.onnx");
        var whisper = Path.Combine(_modelDir, "whisper");
        var matcha = Path.Combine(_modelDir, "male_voxcpm_2.onnx");
        Ready = File.Exists(wake) && Directory.Exists(whisper) && File.Exists(matcha);
        Status = Ready
            ? "hey jarvis, Matcha male_voxcpm_2, 4 kroki"
            : "brak modeli głosu, zostaje tekst na pasku";
    }

    public IEnumerable<string> Speak(string clause)
    {
        if (!Ready || string.IsNullOrWhiteSpace(clause))
            yield break;
        var runner = Path.Combine(_modelDir, "matcha.exe");
        if (!File.Exists(runner))
            yield break;
        yield return clause;
    }

    public void RegisterWakeHotkey(nint hwnd)
    {
        RegisterHotKey(hwnd, 1, 0x0001, 0x20);
    }

    [DllImport("user32.dll")]
    private static extern bool RegisterHotKey(nint hWnd, int id, uint modifiers, uint key);
}
