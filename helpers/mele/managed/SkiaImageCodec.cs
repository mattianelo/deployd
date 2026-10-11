using System;
using System.IO;
using System.Numerics;
using System.Runtime.InteropServices;

using SkiaSharp;

namespace LegendaryExplorerCore.Textures;

internal static class SkiaImageCodec
{
    internal static SKBitmap Decode(Stream stream, SKEncodedImageFormat format, SKColorType color)
    {
        using var codec = SKCodec.Create(stream)
            ?? throw new InvalidDataException("Cannot read texture image.");
        if (codec.EncodedFormat != format)
            throw new InvalidDataException("Texture image does not match its declared format.");
        int width = codec.Info.Width, height = codec.Info.Height;
        if (width <= 0 || height <= 0 || !BitOperations.IsPow2(width) || !BitOperations.IsPow2(height))
            throw new TextureSizeNotPowerOf2Exception();
        var info = PixelInfo(width, height, color);
        var bitmap = new SKBitmap(info);
        try
        {
            if (bitmap.GetPixels() == IntPtr.Zero)
                throw new InvalidDataException("Cannot allocate texture pixels.");
            if (codec.GetPixels(info, bitmap.GetPixels()) != SKCodecResult.Success)
                throw new InvalidDataException("Texture image is incomplete or invalid.");
            return bitmap;
        }
        catch
        {
            bitmap.Dispose();
            throw;
        }
    }

    internal static SKBitmap FromPixels(byte[] pixels, int width, int height, SKColorType color)
    {
        var info = PixelInfo(width, height, color);
        if (pixels.Length != info.BytesSize)
            throw new InvalidDataException("Texture pixel count does not match its dimensions.");
        var bitmap = new SKBitmap(info);
        try
        {
            if (bitmap.GetPixels() == IntPtr.Zero)
                throw new InvalidDataException("Cannot allocate texture pixels.");
            Marshal.Copy(pixels, 0, bitmap.GetPixels(), pixels.Length);
            return bitmap;
        }
        catch
        {
            bitmap.Dispose();
            throw;
        }
    }

    private static SKImageInfo PixelInfo(int width, int height, SKColorType color)
    {
        if (width <= 0 || height <= 0 || (long)width * height > int.MaxValue / 4)
            throw new InvalidDataException("Texture dimensions exceed the supported pixel buffer size.");
        // Texture channels include data and transparent colors, so never premultiply or color-convert them.
        return new SKImageInfo(width, height, color, SKAlphaType.Unpremul);
    }

    internal static byte[] BgrBytes(SKBitmap bitmap)
    {
        byte[] bgra = bitmap.Bytes;
        byte[] bgr = new byte[checked(bitmap.Width * bitmap.Height * 3)];
        for (int source = 0, target = 0; source < bgra.Length; source += 4, target += 3)
            Array.Copy(bgra, source, bgr, target, 3);
        return bgr;
    }
}
