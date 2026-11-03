using System.ComponentModel;
using System.Runtime.CompilerServices;
using Microsoft.UI.Xaml;

namespace JarvisDesktop;

public sealed class ChatLine : INotifyPropertyChanged
{
    private string _text = "";

    public string Role { get; set; } = "";

    public string Text
    {
        get => _text;
        set
        {
            if (_text == value) return;
            _text = value;
            OnPropertyChanged();
        }
    }

    public bool IsJarvis => Role == "jarvis";

    public Visibility JarvisVisibility => IsJarvis ? Visibility.Visible : Visibility.Collapsed;

    public Visibility MineVisibility => IsJarvis ? Visibility.Collapsed : Visibility.Visible;

    public event PropertyChangedEventHandler? PropertyChanged;

    private void OnPropertyChanged([CallerMemberName] string? name = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}
