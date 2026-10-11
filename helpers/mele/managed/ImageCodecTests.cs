using System;
using System.IO;
using System.Linq;

using BCnEncoder.Encoder;
using BCnEncoder.Shared;
using SkiaSharp;
using LegendaryExplorerCore.Textures;

using TextureImage = LegendaryExplorerCore.Textures.Image;
using TextureFormat = LegendaryExplorerCore.Textures.PixelFormat;

namespace Deployd.Mele;

internal static class ImageCodecTests
{
    internal static void Run()
    {
        PreservesPngChannelsAndTransparentColors();
        WritesBgraAsPng();
        DecodesJpegAsBgr();
        RejectsInvalidImages();
        PreservesBlockCompressionAndMipmaps();
        Console.WriteLine("Image codec tests passed (PNG, JPEG, alpha, invalid input, BC5/BC7 and mipmaps).");
    }

    private static readonly byte[] Pixels = {
        231, 17, 43, 255, 29, 219, 61, 128,
        47, 71, 203, 0, 101, 131, 151, 1,
    };

    private static void PreservesPngChannelsAndTransparentColors()
    {
        byte[] png = Convert.FromBase64String("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAGklEQVR4nGN4Lqj9X/a2bQODvvtphtTm6YwAQMMGmQ8fdngAAAAASUVORK5CYII=");
        var image = new TextureImage(png, TextureImage.ImageFormat.PNG);
        Require(image.mipMaps.Count == 1 && image.mipMaps[0].width == 2 && image.mipMaps[0].height == 2,
            "PNG dimensions changed.");
        Require(image.mipMaps[0].data.SequenceEqual(Pixels), "PNG RGBA channels or transparent colors changed.");
    }

    private static void WritesBgraAsPng()
    {
        byte[] bgra = Pixels.ToArray();
        for (int i = 0; i < bgra.Length; i += 4)
            (bgra[i], bgra[i + 2]) = (bgra[i + 2], bgra[i]);
        using var png = TextureImage.convertToPng(bgra, 2, 2, TextureFormat.ARGB);
        Require(png.Position == 0, "PNG output is not rewound.");
        var decoded = new TextureImage(png.ToArray(), TextureImage.ImageFormat.PNG);
        Require(decoded.mipMaps[0].data.SequenceEqual(Pixels), "PNG output swapped channels or premultiplied alpha.");
        Reject(() => TextureImage.convertToPng(new byte[3], 2, 2, TextureFormat.ARGB));
    }

    private static void DecodesJpegAsBgr()
    {
        using var bitmap = new SKBitmap(new SKImageInfo(8, 8, SKColorType.Rgba8888, SKAlphaType.Opaque));
        bitmap.Erase(new SKColor(210, 80, 30));
        using var encoded = bitmap.Encode(SKEncodedImageFormat.Jpeg, 100);
        var image = new TextureImage(encoded.ToArray(), TextureImage.ImageFormat.JPEG);
        byte[] bgr = image.mipMaps[0].data;
        Require(image.pixelFormat == TextureFormat.RGB && bgr.Length == 8 * 8 * 3, "JPEG pixel layout changed.");
        for (int i = 0; i < bgr.Length; i += 3)
            Require(Math.Abs(bgr[i] - 30) <= 3 && Math.Abs(bgr[i + 1] - 80) <= 3 && Math.Abs(bgr[i + 2] - 210) <= 3,
                "JPEG channels changed.");
    }

    private static void RejectsInvalidImages()
    {
        Reject(() => new TextureImage(new byte[] { 1, 2, 3 }, TextureImage.ImageFormat.PNG));
        byte[] png = Convert.FromBase64String("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAGklEQVR4nGN4Lqj9X/a2bQODvvtphtTm6YwAQMMGmQ8fdngAAAAASUVORK5CYII=");
        Reject(() => new TextureImage(png[..(png.Length / 2)], TextureImage.ImageFormat.PNG));
        Reject(() => new TextureImage(png, TextureImage.ImageFormat.JPEG));
        using var bitmap = new SKBitmap(3, 2);
        using var encoded = bitmap.Encode(SKEncodedImageFormat.Png, 100);
        try { _ = new TextureImage(encoded.ToArray(), TextureImage.ImageFormat.PNG); }
        catch (TextureSizeNotPowerOf2Exception) { return; }
        throw new InvalidOperationException("Non-power-of-two texture was accepted.");
    }

    private static void PreservesBlockCompressionAndMipmaps()
    {
        foreach (var format in new[] { TextureFormat.BC5, TextureFormat.BC7 })
        {
            byte[] rgba = Enumerable.Range(0, 8 * 8 * 4).Select(i => (byte)(i * 13)).ToArray();
            var image = TextureImage.LoadFromRaw(rgba, TextureFormat.ARGB, 8, 8);
            image.correctMips(format);
            var encoder = new BcEncoder();
            encoder.OutputOptions.GenerateMipMaps = false;
            encoder.OutputOptions.Quality = CompressionQuality.Balanced;
            encoder.OutputOptions.Format = format == TextureFormat.BC5 ? CompressionFormat.Bc5 : CompressionFormat.Bc7;
            byte[] expected = encoder.EncodeToRawBytes(rgba, 8, 8, BCnEncoder.Encoder.PixelFormat.Rgba32, 0, out _, out _);
            Require(image.mipMaps[0].data.SequenceEqual(expected), "Texture compression changed channel order or settings.");
            Require(image.mipMaps.Count == 4, "Mip chain does not reach 1x1.");
            for (int i = 0; i < 4; i++)
            {
                int dimension = 8 >> i;
                var mip = image.mipMaps[i];
                Require(mip.origWidth == dimension && mip.origHeight == dimension, "Mip dimensions changed.");
                Require(mip.data.Length == (i == 0 ? 64 : 16), "Sub-4x4 mip is not a complete compression block.");
            }
        }
    }

    private static void Reject(Action action)
    {
        try { action(); }
        catch (InvalidDataException) { return; }
        throw new InvalidOperationException("Invalid texture was accepted.");
    }

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }
}
