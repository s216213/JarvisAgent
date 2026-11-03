using Microsoft.UI.Xaml;

namespace JarvisDesktop;

public sealed partial class SpaceWindow : Window
{
    public SpaceWindow()
    {
        InitializeComponent();
        var url = BrainSession.ApiBase.TrimEnd('/') + "/space";
        try
        {
            View.Source = new Uri(url);
        }
        catch
        {
            /* brak WebView2 zostawia puste okno */
        }
    }
}
