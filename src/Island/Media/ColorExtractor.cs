using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace Island.Media;

/// <summary>
/// 从专辑封面里取一个主题色，用来给进度条和律动条上色（按饱和度加权，颜色更鲜明）。
/// </summary>
internal static class ColorExtractor
{
    private const int SampleSize = 12;

    public static Color Accent(BitmapSource image, Color fallback)
    {
        try
        {
            // 缩到很小再统计，几乎没有开销
            var scaled = new TransformedBitmap(image,
                new ScaleTransform((double)SampleSize / image.PixelWidth, (double)SampleSize / image.PixelHeight));
            var bgra = new FormatConvertedBitmap(scaled, PixelFormats.Bgra32, null, 0);

            int width = bgra.PixelWidth, height = bgra.PixelHeight;
            if (width == 0 || height == 0) return fallback;

            int stride = width * 4;
            var pixels = new byte[stride * height];
            bgra.CopyPixels(new Int32Rect(0, 0, width, height), pixels, stride, 0);

            // 按饱和度加权求平均：灰色、黑色、白色像素对结果影响很小，颜色更鲜明
            double r = 0, g = 0, b = 0, weightSum = 0;
            for (int i = 0; i < pixels.Length; i += 4)
            {
                double pb = pixels[i] / 255.0, pg = pixels[i + 1] / 255.0, pr = pixels[i + 2] / 255.0;
                double alpha = pixels[i + 3] / 255.0;
                double max = Math.Max(pr, Math.Max(pg, pb));
                double min = Math.Min(pr, Math.Min(pg, pb));
                double saturation = max <= 0 ? 0 : (max - min) / max;
                double weight = alpha * (0.05 + saturation * saturation) * (0.2 + max);

                r += pr * weight;
                g += pg * weight;
                b += pb * weight;
                weightSum += weight;
            }

            if (weightSum <= 0) return fallback;
            // 这里返回原始颜色；显示前再按当前主题的背景调整可读性（见 ColorMath.EnsureReadable）
            return Color.FromRgb(ToByte(r / weightSum), ToByte(g / weightSum), ToByte(b / weightSum));
        }
        catch
        {
            return fallback;
        }
    }

    private static byte ToByte(double v) => (byte)Math.Round(Math.Clamp(v, 0, 1) * 255);
}
