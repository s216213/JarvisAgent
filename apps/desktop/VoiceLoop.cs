using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using Windows.Globalization;
using Windows.Media.Playback;
using Windows.Media.SpeechRecognition;
using Windows.Media.SpeechSynthesis;

namespace JarvisDesktop;

public sealed class VoiceLoop
{
    private const uint WmHotkey = 0x0312;
    private readonly string _modelDir;
    private readonly Queue<string> _say = new();
    private readonly object _gate = new();
    private int _saying;
    private int _listening;
    private SubclassProc? _proc;
    private MediaPlayer? _player;

    public string Status { get; private set; } = "pobieram głos obok programu";
    public event Action<string>? Heard;
    public event Action<string>? StatusChanged;

    public VoiceLoop()
    {
        _modelDir = Path.Combine(AppContext.BaseDirectory, "models");
        _ = EnsureAsync();
    }

    public void Attach(nint hwnd)
    {
        _proc = Subclass;
        SetWindowSubclass(hwnd, _proc, 1, 0);
        RegisterHotKey(hwnd, 1, 0x0001, 0x20);
    }

    public IEnumerable<string> Speak(string clause)
    {
        if (string.IsNullOrWhiteSpace(clause))
            yield break;
        lock (_gate)
            _say.Enqueue(clause.Trim());
        if (Interlocked.CompareExchange(ref _saying, 1, 0) == 0)
            _ = PumpAsync();
        yield return clause;
    }

    private async Task EnsureAsync()
    {
        try
        {
            Directory.CreateDirectory(_modelDir);
            if (Find("sherpa-onnx-offline.exe") is null)
                await Fetch("https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-win-x64-shared.tar.bz2");
            if (Find("tiny-encoder.int8.onnx") is null)
                await Fetch("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-tiny.tar.bz2");
            if (Find("pl_PL-gosia-medium.onnx") is null)
                await Fetch("https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-pl_PL-gosia-medium.tar.bz2");
            if (Find("sherpa-onnx-keyword-spotter.exe") is null)
                await Fetch("https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01.tar.bz2");
            SetStatus(Ready()
                ? "Alt+Space, Whisper i Piper obok programu"
                : "zostaje mowa Windows");
            StartSpotter();
        }
        catch (Exception ex)
        {
            SetStatus("zostaje mowa Windows: " + ex.Message);
        }
    }

    private bool Ready() => Find("sherpa-onnx-offline.exe") is not null && Find("tiny-encoder.int8.onnx") is not null && Find("pl_PL-gosia-medium.onnx") is not null;

    private async Task Fetch(string url)
    {
        var archive = Path.Combine(_modelDir, Path.GetFileName(new Uri(url).AbsolutePath));
        using var http = new HttpClient();
        http.DefaultRequestHeaders.UserAgent.ParseAdd("JarvisDesktop");
        http.Timeout = TimeSpan.FromMinutes(15);
        await using (var input = await http.GetStreamAsync(url))
        await using (var output = File.Create(archive))
            await input.CopyToAsync(output);
        var tar = Path.Combine(Environment.SystemDirectory, "tar.exe");
        var process = Process.Start(new ProcessStartInfo(tar, $"-xjf \"{archive}\" -C \"{_modelDir}\"")
        {
            CreateNoWindow = true,
            UseShellExecute = false,
        });
        if (process is not null)
            await process.WaitForExitAsync();
        File.Delete(archive);
    }

    private async Task PumpAsync()
    {
        try
        {
            while (true)
            {
                string? clause;
                lock (_gate)
                {
                    if (!_say.TryDequeue(out clause))
                        break;
                }
                if (clause is not null)
                    await SayOne(clause);
            }
        }
        finally
        {
            Interlocked.Exchange(ref _saying, 0);
            lock (_gate)
            {
                if (_say.Count > 0 && Interlocked.CompareExchange(ref _saying, 1, 0) == 0)
                    _ = PumpAsync();
            }
        }
    }

    private async Task SayOne(string clause)
    {
        var tts = Find("sherpa-onnx-offline-tts.exe");
        var model = Find("pl_PL-gosia-medium.onnx");
        var tokens = model is null ? null : Path.Combine(Path.GetDirectoryName(model)!, "tokens.txt");
        var data = model is null ? null : Path.Combine(Path.GetDirectoryName(model)!, "espeak-ng-data");
        if (tts is not null && model is not null && tokens is not null && Directory.Exists(data))
        {
            var wav = Path.Combine(Path.GetTempPath(), "jarvis-say.wav");
            var args = $"--vits-model=\"{model}\" --vits-tokens=\"{tokens}\" --vits-data-dir=\"{data}\" --output-filename=\"{wav}\" \"{clause.Replace("\"", "")}\"";
            if (await Run(tts, args) && File.Exists(wav))
            {
                Play(new Uri(wav));
                return;
            }
        }
        var synth = new SpeechSynthesizer();
        var voice = SpeechSynthesizer.AllVoices.FirstOrDefault(item => item.Language.StartsWith("pl", StringComparison.OrdinalIgnoreCase));
        if (voice is not null)
            synth.Voice = voice;
        var stream = await synth.SynthesizeTextToStreamAsync(clause);
        _player ??= new MediaPlayer();
        _player.SetStreamSource(stream);
        _player.Play();
    }

    private void Play(Uri wav)
    {
        _player ??= new MediaPlayer();
        _player.SetUriSource(wav);
        _player.Play();
    }

    private void StartSpotter()
    {
        var exe = Find("sherpa-onnx-keyword-spotter.exe");
        var joiner = Directory.Exists(_modelDir)
            ? Directory.EnumerateFiles(_modelDir, "joiner*.onnx", SearchOption.AllDirectories).FirstOrDefault()
            : null;
        if (exe is null || joiner is null)
            return;
        var dir = Path.GetDirectoryName(joiner)!;
        var tokens = Path.Combine(dir, "tokens.txt");
        var keywords = Path.Combine(dir, "keywords.txt");
        var encoder = Directory.EnumerateFiles(dir, "encoder*.onnx").FirstOrDefault();
        var decoder = Directory.EnumerateFiles(dir, "decoder*.onnx").FirstOrDefault();
        if (encoder is null || decoder is null || !File.Exists(tokens) || !File.Exists(keywords))
            return;
        try
        {
            var args = $"--tokens=\"{tokens}\" --encoder=\"{encoder}\" --decoder=\"{decoder}\" --joiner=\"{joiner}\" --keywords-file=\"{keywords}\"";
            var process = new Process
            {
                StartInfo = Info(exe, args),
                EnableRaisingEvents = true,
            };
            process.OutputDataReceived += (_, message) =>
            {
                if (message.Data is not null && message.Data.Contains("jarvis", StringComparison.OrdinalIgnoreCase))
                    _ = ListenAsync();
            };
            if (!process.Start())
                return;
            process.BeginOutputReadLine();
            process.BeginErrorReadLine();
        }
        catch
        {
            /* zostaje Alt+Space */
        }
    }

    private async Task ListenAsync()
    {
        if (Interlocked.CompareExchange(ref _listening, 1, 0) != 0)
            return;
        try
        {
            var offline = Find("sherpa-onnx-offline.exe");
            var encoder = Find("tiny-encoder.int8.onnx");
            var decoder = Find("tiny-decoder.int8.onnx");
            var tokens = encoder is null ? null : Path.Combine(Path.GetDirectoryName(encoder)!, "tiny-tokens.txt");
            if (offline is not null && encoder is not null && decoder is not null && tokens is not null && File.Exists(tokens))
            {
                var wav = Path.Combine(Path.GetTempPath(), "jarvis-hear.wav");
                if (Record(wav))
                {
                    var args = $"--tokens=\"{tokens}\" --whisper-encoder=\"{encoder}\" --whisper-decoder=\"{decoder}\" --whisper-language=pl --num-threads=2 \"{wav}\"";
                    var text = await Capture(offline, args);
                    var line = text.Split('\n', StringSplitOptions.RemoveEmptyEntries).LastOrDefault(item => !item.StartsWith("kod "))?.Trim();
                    if (!string.IsNullOrWhiteSpace(line))
                        Heard?.Invoke(line);
                    return;
                }
            }
            var recognizer = new SpeechRecognizer(new Language("pl-PL"));
            recognizer.Constraints.Add(new SpeechRecognitionTopicConstraint(SpeechRecognitionScenario.Dictation, "mowa"));
            await recognizer.CompileConstraintsAsync();
            var result = await recognizer.RecognizeAsync();
            if (!string.IsNullOrWhiteSpace(result.Text))
                Heard?.Invoke(result.Text.Trim());
        }
        catch (Exception ex)
        {
            SetStatus("mikrofon: " + ex.Message);
        }
        finally
        {
            Interlocked.Exchange(ref _listening, 0);
        }
    }

    private static bool Record(string wav)
    {
        var name = "jarvisrec";
        if (mciSendString($"open new type waveaudio alias {name}", null, 0, 0) != 0)
            return false;
        mciSendString($"set {name} time format ms bitspersample 16 channels 1 samplespersec 16000", null, 0, 0);
        mciSendString($"record {name}", null, 0, 0);
        Thread.Sleep(4000);
        var saved = mciSendString($"save {name} \"{wav}\"", null, 0, 0) == 0;
        mciSendString($"close {name}", null, 0, 0);
        return saved && File.Exists(wav);
    }

    private async Task<bool> Run(string exe, string args)
    {
        var process = Process.Start(Info(exe, args));
        if (process is null)
            return false;
        await process.WaitForExitAsync();
        return process.ExitCode == 0;
    }

    private async Task<string> Capture(string exe, string args)
    {
        var process = new Process { StartInfo = Info(exe, args) };
        if (!process.Start())
            return "";
        var stdout = await process.StandardOutput.ReadToEndAsync();
        var stderr = await process.StandardError.ReadToEndAsync();
        await process.WaitForExitAsync();
        return stdout + "\n" + stderr;
    }

    private static ProcessStartInfo Info(string exe, string args)
    {
        var info = new ProcessStartInfo(exe, args)
        {
            CreateNoWindow = true,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            WorkingDirectory = Path.GetDirectoryName(exe) ?? "",
        };
        var path = Environment.GetEnvironmentVariable("PATH") ?? "";
        info.Environment["PATH"] = info.WorkingDirectory + Path.PathSeparator + path;
        return info;
    }

    private string? Find(string name)
    {
        if (!Directory.Exists(_modelDir))
            return null;
        return Directory.EnumerateFiles(_modelDir, name, SearchOption.AllDirectories).FirstOrDefault();
    }

    private void SetStatus(string text)
    {
        Status = text;
        StatusChanged?.Invoke(text);
    }

    private nint Subclass(nint hwnd, uint msg, nint wParam, nint lParam, nuint id, nuint data)
    {
        if (msg == WmHotkey)
        {
            _ = ListenAsync();
            return 0;
        }
        return DefSubclassProc(hwnd, msg, wParam, lParam);
    }

    private delegate nint SubclassProc(nint hwnd, uint msg, nint wParam, nint lParam, nuint id, nuint data);

    [DllImport("comctl32.dll")]
    private static extern bool SetWindowSubclass(nint hwnd, SubclassProc callback, nuint id, nuint data);

    [DllImport("comctl32.dll")]
    private static extern nint DefSubclassProc(nint hwnd, uint msg, nint wParam, nint lParam);

    [DllImport("user32.dll")]
    private static extern bool RegisterHotKey(nint hwnd, int id, uint modifiers, uint key);

    [DllImport("winmm.dll", CharSet = CharSet.Unicode)]
    private static extern int mciSendString(string command, StringBuilder? ret, int length, nint callback);
}
