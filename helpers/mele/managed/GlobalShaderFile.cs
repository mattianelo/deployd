using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;

using LegendaryExplorerCore.Packages;

namespace Deployd.Mele;

internal sealed record ShaderSpan(int Start, int Pointer, int Code, int Length, int End);

internal sealed class GlobalShaderFile
{
    internal const int Limit = 64 * 1024 * 1024;
    private readonly byte[] data;
    internal readonly ShaderSpan[] Shaders;
    private readonly MEGame game;

    internal GlobalShaderFile(byte[] data, MEGame game)
    {
        this.data = data;
        this.game = game;
        if (data.Length is < 25 or > Limit || !data.AsSpan(0, 4).SequenceEqual("BMSG"u8))
            throw new InvalidDataException("Invalid global shader cache header.");
        using var stream = new MemoryStream(data, writable: false);
        using var reader = new BinaryReader(stream);
        stream.Position = 4;
        if (reader.ReadInt32() != UnrealPackageFile.UnrealVersion(game)
            || reader.ReadInt32() != UnrealPackageFile.LicenseeVersion(game) || reader.ReadByte() != 5)
            throw new InvalidDataException("Global shader cache does not match the selected Legendary Edition game.");
        int Count()
        {
            int count = reader.ReadInt32();
            if (count < 0 || count > 65536 || count > (stream.Length - stream.Position) / 4)
                throw new InvalidDataException("Invalid global shader cache table count.");
            return count;
        }
        for (int count = Count(); count > 0; count--) { Name(reader); Skip(reader, 4); }
        for (int count = Count(); count > 0; count--) Name(reader);
        int shaderCount = Count();
        if (shaderCount == 0) throw new InvalidDataException("Global shader cache contains no shaders.");
        var shaders = new List<ShaderSpan>();
        for (int index = 0; index < shaderCount; index++)
        {
            int start = checked((int)stream.Position);
            string type = Name(reader);
            byte[] id = reader.ReadBytes(16);
            int pointer = checked((int)stream.Position);
            int end = reader.ReadInt32();
            if (end <= stream.Position || end > data.Length)
                throw new InvalidDataException("Global shader record extends outside the cache.");
            byte platform = reader.ReadByte();
            byte frequency = reader.ReadByte();
            if (platform > 5 || frequency > 5)
                throw new InvalidDataException("Unsupported global shader platform or frequency.");
            int prefix = type switch
            {
                "FBinkYCrCbAToRGBAPixelShader" => 6,
                "FBinkYCrCbToRGBNoPixelAlphaPixelShader" => 48,
                _ => 0,
            };
            Skip(reader, prefix);
            int length = reader.ReadInt32();
            int code = checked((int)stream.Position);
            if (length < 1 || length > ShaderMerge.BytecodeLimit || length > end - code)
                throw new InvalidDataException("Invalid global shader bytecode extent.");
            Skip(reader, length + 4);
            if (!reader.ReadBytes(16).AsSpan().SequenceEqual(id) || Name(reader) != type)
                throw new InvalidDataException("Global shader identity is inconsistent.");
            Skip(reader, 4);
            if (stream.Position > end) throw new InvalidDataException("Overlapping global shader records.");
            stream.Position = end;
            shaders.Add(new(start, pointer, code, length, end));
        }
        for (int count = Count(); count > 0; count--)
        {
            string name = Name(reader);
            Skip(reader, 16);
            if (Name(reader) != name) throw new InvalidDataException("Invalid vertex factory identity.");
        }
        if (stream.Position != stream.Length && !(stream.Position + 1 == stream.Length && reader.ReadByte() == 0))
            throw new InvalidDataException("Unexpected global shader cache trailing data.");
        Shaders = shaders.ToArray();
    }

    private static void Skip(BinaryReader reader, int count)
    {
        if (count < 0 || count > reader.BaseStream.Length - reader.BaseStream.Position)
            throw new InvalidDataException("Truncated global shader cache.");
        reader.BaseStream.Position += count;
    }

    private static string Name(BinaryReader reader)
    {
        int count = reader.ReadInt32();
        if (count == 0 || count is < -4096 or > 4096)
            throw new InvalidDataException("Invalid global shader name length.");
        int size = count < 0 ? -count * 2 : count;
        if (size > reader.BaseStream.Length - reader.BaseStream.Position)
            throw new InvalidDataException("Truncated global shader name.");
        byte[] bytes = reader.ReadBytes(size);
        if (bytes[^1] != 0 || (count < 0 && bytes[^2] != 0))
            throw new InvalidDataException("Unterminated global shader name.");
        return count < 0 ? new UnicodeEncoding(false, false, true).GetString(bytes, 0, size - 2)
            : Encoding.Latin1.GetString(bytes, 0, size - 1);
    }

    internal byte[] Bytecode(int index)
    {
        if (index < 0 || index >= Shaders.Length) throw new InvalidDataException("M3GS shader index is outside the original cache.");
        var shader = Shaders[index];
        return data.AsSpan(shader.Code, shader.Length).ToArray();
    }

    internal byte[] Replace(IReadOnlyDictionary<int, byte[]> replacements, CancellationToken cancellation)
    {
        long size = data.Length;
        foreach (var (index, bytes) in replacements)
        {
            Bytecode(index);
            ShaderMerge.ValidateBytecode(bytes);
            size += bytes.Length - Shaders[index].Length;
        }
        if (size > Limit) throw new InvalidDataException("Merged global shader cache exceeds its size limit.");
        // Preserve opaque shader parameters and name maps that full cache serialization can discard.
        using var stream = new MemoryStream(checked((int)size));
        using var writer = new BinaryWriter(stream);
        int cursor = 0;
        foreach (var shader in Shaders.Select((span, index) => (span, index)))
        {
            cancellation.ThrowIfCancellationRequested();
            var span = shader.span;
            var bytes = replacements.TryGetValue(shader.index, out var replacement) ? replacement : Bytecode(shader.index);
            writer.Write(data, cursor, span.Pointer - cursor);
            int shift = checked((int)stream.Position) - span.Pointer;
            writer.Write(checked(span.End + shift + bytes.Length - span.Length));
            writer.Write(data, span.Pointer + 4, span.Code - 4 - (span.Pointer + 4));
            writer.Write(bytes.Length);
            writer.Write(bytes);
            writer.Write(data, span.Code + span.Length, span.End - span.Code - span.Length);
            cursor = span.End;
        }
        writer.Write(data, cursor, data.Length - cursor);
        byte[] output = stream.ToArray();
        var reopened = new GlobalShaderFile(output, game);
        if (reopened.Shaders.Length != Shaders.Length)
            throw new InvalidDataException("Global shader count changed during serialization.");
        for (int index = 0; index < Shaders.Length; index++)
            if (!reopened.Bytecode(index).AsSpan().SequenceEqual(replacements.TryGetValue(index, out var bytes) ? bytes : Bytecode(index)))
                throw new InvalidDataException("Global shader output differs from the requested replacement.");
        return output;
    }
}
