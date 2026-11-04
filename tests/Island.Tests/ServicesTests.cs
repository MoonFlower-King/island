using System.IO;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Island.Media;
using Island.Services;

namespace Island.Tests;

public class ServicesTests
{
    [Theory]
    [InlineData(0, "0 B/s")]
    [InlineData(512, "512 B/s")]
    [InlineData(2048, "2.0 KB/s")]
    [InlineData(5 * 1024 * 1024, "5.0 MB/s")]
    public void FormatsNetworkSpeed(double bytesPerSecond, string expected) =>
        Assert.Equal(expected, SystemMonitor.FormatSpeed(bytesPerSecond));

    [Fact]
    public void SystemSamplesAreWithinRange()
    {
        var monitor = new SystemMonitor();
        monitor.Sample();
        Thread.Sleep(200);
        SystemSample s = monitor.Sample();

        Assert.InRange(s.CpuPercent, 0, 100);
        Assert.InRange(s.MemoryPercent, 0.1, 100);
        Assert.True(s.MemoryTotalBytes > 0);
        Assert.True(s.DownloadBytesPerSecond >= 0 && s.UploadBytesPerSecond >= 0);
    }

    [Fact]
    public void ClipboardPreviewIsSingleLineAndTruncated()
    {
        var entry = new ClipboardEntry { Text = "第一行\r\n第二行\t" + new string('字', 200) };
        Assert.DoesNotContain('\n', entry.Preview);
        Assert.StartsWith("第一行 第二行", entry.Preview);
        Assert.True(entry.Preview.Length <= 121);
        Assert.EndsWith("…", entry.Preview);
    }

    [Fact]
    public void ShelfItemDescribesFilesAndFolders()
    {
        string dir = Path.Combine(Path.GetTempPath(), "island-tests-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        string file = Path.Combine(dir, "photo.png");
        File.WriteAllBytes(file, new byte[2048]);
        try
        {
            var fileItem = new ShelfItem { Path = file };
            Assert.Equal("photo.png", fileItem.Name);
            Assert.False(fileItem.IsFolder);
            Assert.Equal("2 KB", fileItem.Detail);
            Assert.Equal("", fileItem.Glyph);

            var folderItem = new ShelfItem { Path = dir + Path.DirectorySeparatorChar };
            Assert.True(folderItem.IsFolder);
            Assert.Equal(Path.GetFileName(dir), folderItem.Name);
        }
        finally
        {
            Directory.Delete(dir, recursive: true);
        }

        Assert.False(new ShelfItem { Path = file }.Exists);
    }

    [Fact]
    public void AccentColorKeepsTheArtworkHue()
    {
        BitmapSource image = SolidImage(Color.FromRgb(20, 30, 90));
        Color accent = ColorExtractor.Accent(image, Colors.Green);

        Assert.True(accent.B > accent.R && accent.B > accent.G, $"应保留封面的蓝色调：{accent}");
        Assert.InRange(accent.B, 80, 100);
    }

    private static BitmapSource SolidImage(Color color)
    {
        const int size = 64;
        var pixels = new byte[size * size * 4];
        for (int i = 0; i < pixels.Length; i += 4)
        {
            pixels[i] = color.B;
            pixels[i + 1] = color.G;
            pixels[i + 2] = color.R;
            pixels[i + 3] = 255;
        }
        var bitmap = BitmapSource.Create(size, size, 96, 96, PixelFormats.Bgra32, null, pixels, size * 4);
        bitmap.Freeze();
        return bitmap;
    }
}
