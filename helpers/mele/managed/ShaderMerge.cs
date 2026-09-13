using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Threading;

using LegendaryExplorerCore.Packages;

namespace Deployd.Mele;

internal sealed record ShaderContribution(string Dlc, int Mount, uint Index, InputFile Shader);
internal sealed record ShaderRequest(int Protocol, string Operation, string GameRoot, string OriginalRoot,
    string InputRoot, string OutputRoot, string Game, TargetPackage Target, ShaderContribution[] Contributions);

internal static class ShaderMerge
{
    internal const string Target = "CookedPCConsole/GlobalShaderCache-PC-D3D-SM5.bin";
    internal const int BytecodeLimit = 16 * 1024 * 1024;

    internal static ShaderRequest ReadRequest(string path)
    {
        var request = TransformProtocol.ReadJson<ShaderRequest>(path);
        Validate(request);
        return request;
    }

    private static MEGame Game(ShaderRequest request) => request.Game switch
    {
        "LE1" => MEGame.LE1, "LE2" => MEGame.LE2, "LE3" => MEGame.LE3,
        _ => throw new InvalidDataException("M3GS requires a Legendary Edition game."),
    };

    internal static void Validate(ShaderRequest request)
    {
        Game(request);
        if (request.Protocol != 1 || request.Operation != "mele-m3gs"
            || request.Target?.Original?.Path != Target || request.Target.Current?.Path != Target
            || request.Contributions is null || request.Contributions.Length > 4096)
            throw new InvalidDataException("Invalid M3GS merge request.");
        foreach (string root in new[] { request.GameRoot, request.OriginalRoot, request.InputRoot, request.OutputRoot }) TransformProtocol.ValidateRoot(root);
        if (new[] { request.GameRoot, request.OriginalRoot, request.InputRoot }.Any(root => TransformProtocol.Overlaps(root, request.OutputRoot))
            || TransformProtocol.Overlaps(request.OriginalRoot, request.InputRoot) || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any())
            throw new InvalidDataException("M3GS output must be empty and separate; originals and candidates must be separate.");
        var mounts = new Dictionary<int, string>();
        var dlcs = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var indices = new HashSet<(string, uint)>();
        long total = 0;
        foreach (var input in new[] { request.Target.Original, request.Target.Current })
        {
            if (input.Size is < 25 or > GlobalShaderFile.Limit) throw new InvalidDataException("Invalid global shader cache size.");
            total += input.Size;
        }
        foreach (var item in request.Contributions)
        {
            if (item?.Dlc is null || item.Mount < 0 || !item.Dlc.StartsWith("DLC_MOD_", StringComparison.OrdinalIgnoreCase)
                || item.Dlc.Length > 255 || item.Dlc.Any(ch => !char.IsAsciiLetterOrDigit(ch) && ch != '_')
                || item.Shader?.Path is null || item.Shader.Size is < 32 or > BytecodeLimit)
                throw new InvalidDataException("Invalid M3GS contribution.");
            string path = TransformProtocol.Relative(item.Shader.Path);
            string filename = Path.GetFileName(path);
            var tokens = filename.Split('-');
            if (!path.StartsWith($"DLC/{item.Dlc}/CookedPCConsole/", StringComparison.OrdinalIgnoreCase)
                || path.Split('/').Length != 4 || tokens.Length < 3
                || !tokens[0].Equals("GlobalShader", StringComparison.OrdinalIgnoreCase)
                || !filename.EndsWith(".m3gs", StringComparison.OrdinalIgnoreCase)
                || filename.Length <= tokens[0].Length + tokens[1].Length + 7
                || !uint.TryParse(tokens[1], NumberStyles.None, CultureInfo.InvariantCulture, out uint index)
                || index > int.MaxValue || index != item.Index)
                throw new InvalidDataException("M3GS requires a GlobalShader-<index>-<name>.m3gs file directly inside its DLC.");
            if (!indices.Add((item.Dlc.ToLowerInvariant(), item.Index)))
                throw new InvalidDataException($"Multiple M3GS files replace shader {item.Index} within DLC '{item.Dlc}'.");
            if ((dlcs.TryGetValue(item.Dlc, out int priority) && priority != item.Mount)
                || (mounts.TryGetValue(item.Mount, out string owner) && !owner.Equals(item.Dlc, StringComparison.OrdinalIgnoreCase)))
                throw new InvalidDataException("Ambiguous M3GS DLC mount order.");
            dlcs[item.Dlc] = item.Mount;
            mounts[item.Mount] = item.Dlc;
            total += item.Shader.Size;
        }
        if (total > 512 * 1024 * 1024) throw new InvalidDataException("M3GS inputs exceed the request limit.");
    }

    internal static void ValidateBytecode(byte[] data)
    {
        if (data.Length is < 32 or > BytecodeLimit || !data.AsSpan(0, 4).SequenceEqual("DXBC"u8))
            throw new InvalidDataException("M3GS requires compiled DXBC shader bytecode.");
        uint Word(int offset) => BinaryPrimitives.ReadUInt32LittleEndian(data.AsSpan(offset, 4));
        uint count = Word(28);
        if (Word(20) != 1 || Word(24) != data.Length || count == 0 || count > (data.Length - 32) / 4)
            throw new InvalidDataException("Invalid M3GS DXBC header.");
        var chunks = new List<(uint Start, uint End)>();
        for (int index = 0; index < count; index++)
        {
            uint offset = Word(32 + index * 4);
            if (offset < 32 + count * 4 || offset > data.Length - 8)
                throw new InvalidDataException("M3GS chunk is outside its payload.");
            uint size = Word((int)offset + 4);
            if (size > data.Length - offset - 8) throw new InvalidDataException("Truncated M3GS chunk payload.");
            chunks.Add((offset, offset + 8 + size));
        }
        var ordered = chunks.OrderBy(chunk => chunk.Start).ToArray();
        for (int index = 1; index < ordered.Length; index++)
            if (ordered[index - 1].End > ordered[index].Start) throw new InvalidDataException("Overlapping M3GS chunks.");
    }

    internal static OutputFile[] Execute(ShaderRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        cancellation.ThrowIfCancellationRequested();
        using var original = TransformProtocol.ReadVerified(request.OriginalRoot, request.Target.Original, GlobalShaderFile.Limit);
        using var current = TransformProtocol.ReadVerified(request.InputRoot, request.Target.Current, GlobalShaderFile.Limit);
        var cache = new GlobalShaderFile(original.ToArray(), Game(request));
        var replacements = new Dictionary<int, byte[]>();
        int completed = 0;
        foreach (var item in request.Contributions.OrderBy(item => item.Mount).ThenBy(item => item.Index))
        {
            cancellation.ThrowIfCancellationRequested();
            cache.Bytecode(checked((int)item.Index));
            using var source = TransformProtocol.ReadVerified(request.InputRoot, item.Shader, BytecodeLimit);
            byte[] bytes = source.ToArray();
            ValidateBytecode(bytes);
            replacements[(int)item.Index] = bytes;
            progress(++completed, request.Contributions.Length);
        }
        byte[] output = cache.Replace(replacements, cancellation);
        cancellation.ThrowIfCancellationRequested();
        string path = Path.Combine(request.OutputRoot, Target);
        Directory.CreateDirectory(Path.GetDirectoryName(path));
        TransformProtocol.ValidateRoot(Path.GetDirectoryName(path));
        using var destination = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        destination.Write(output);
        destination.Flush(flushToDisk: true);
        return new[] { new OutputFile(Target, output.Length, Convert.ToHexStringLower(SHA256.HashData(output))) };
    }
}
