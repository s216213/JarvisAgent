using System.Diagnostics;
using System.Text.Json;

namespace JarvisDesktop;

public static class RepoPatch
{
    public static string Apply(string body)
    {
        var repo = Environment.GetEnvironmentVariable("JARVIS_REPO");
        if (string.IsNullOrWhiteSpace(repo))
        {
            var marker = Path.Combine(AppContext.BaseDirectory, "jarvis.repo");
            if (File.Exists(marker))
                repo = File.ReadAllText(marker).Trim();
        }
        if (string.IsNullOrWhiteSpace(repo) || !Directory.Exists(repo))
            return "Na tym komputerze nie ma JARVIS_REPO.";
        using var doc = JsonDocument.Parse(body);
        var root = doc.RootElement;
        var written = new List<string>();
        if (root.TryGetProperty("files", out var files) && files.ValueKind == JsonValueKind.Array)
        {
            foreach (var file in files.EnumerateArray())
            {
                var rel = file.TryGetProperty("path", out var pathEl) ? pathEl.GetString() ?? "" : "";
                var content = file.TryGetProperty("content", out var contentEl) ? contentEl.GetString() ?? "" : "";
                if (!Allowed(rel))
                    return "Odrzucam ścieżkę " + rel;
                var full = Path.GetFullPath(Path.Combine(repo, rel.Replace('/', Path.DirectorySeparatorChar)));
                if (!full.StartsWith(Path.GetFullPath(repo), StringComparison.OrdinalIgnoreCase))
                    return "Odrzucam ścieżkę " + rel;
                Directory.CreateDirectory(Path.GetDirectoryName(full)!);
                File.WriteAllText(full, content);
                written.Add(rel);
            }
        }
        if (root.TryGetProperty("diff", out var diffEl) && diffEl.ValueKind == JsonValueKind.String)
        {
            var diff = diffEl.GetString() ?? "";
            if (!string.IsNullOrWhiteSpace(diff))
            {
                var patch = Path.Combine(repo, ".jarvis-proposal.diff");
                File.WriteAllText(patch, diff);
                var output = Git(repo, "apply --whitespace=nowarn \"" + patch + "\"");
                File.Delete(patch);
                if (!output.EndsWith("kod 0", StringComparison.Ordinal))
                    return output;
            }
        }
        if (written.Count == 0 && !(root.TryGetProperty("diff", out var any) && any.GetString()?.Length > 0))
            return "Propozycja nie ma plików.";
        return "Nałożyłem propozycję lokalnie, bez commita.";
    }

    private static bool Allowed(string path)
    {
        var rel = path.Replace('\\', '/');
        if (rel.StartsWith('/') || rel.Contains("..", StringComparison.Ordinal) || rel.Contains(".env", StringComparison.Ordinal) || rel.Contains("server.key", StringComparison.Ordinal))
            return false;
        string[] prefixes = ["apps/core/", "apps/web/", "apps/desktop/", "apps/mobile/", "deploy/", "protocol/", "design/"];
        return prefixes.Any(rel.StartsWith);
    }

    private static string Git(string repo, string args)
    {
        var process = Process.Start(new ProcessStartInfo("git", "-C \"" + repo + "\" " + args)
        {
            CreateNoWindow = true,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        });
        if (process is null)
            return "Nie mogę uruchomić gita.";
        var stdout = process.StandardOutput.ReadToEnd();
        var stderr = process.StandardError.ReadToEnd();
        process.WaitForExit();
        return stdout + stderr + "\nkod " + process.ExitCode;
    }
}
