using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;

namespace Deployd.Mele;

internal sealed record TextureRequest(int Protocol, string Operation, string GameRoot, string InputRoot,
    string OutputRoot, string Game, string Dlc, InputFile[] Manifests, InputFile[] Packages,
    string[] Outputs, int Textures);
internal sealed record TextureManifestDocument(string Game, TextureManifestEntry[] Textures);
internal sealed record TextureManifestEntry(string Sourcepackage, string Textureifp);

internal sealed class TextureManifest
{
    internal MEGame Game { get; init; }
    internal List<TextureOverrideTextureEntry> Textures { get; set; } = new();
}

internal static class TextureCompile
{
    private const long ManifestLimit = 16 * 1024 * 1024;
    private const long PackageLimit = 512 * 1024 * 1024;
    private const long JobLimit = 4L * 1024 * 1024 * 1024;

    internal static TextureRequest ReadRequest(string path)
    {
        var request = TransformProtocol.ReadJson<TextureRequest>(path);
        Validate(request);
        return request;
    }

    internal static void Validate(TextureRequest request)
    {
        if (request.Protocol != 1 || request.Operation != "mele-m3to"
            || request.Game is not ("LE1" or "LE2" or "LE3")
            || request.Dlc is null || !request.Dlc.StartsWith("DLC_MOD_", StringComparison.Ordinal)
            || !MergeDlc.Identifier(request.Dlc) || request.Manifests is null
            || request.Manifests.Length is < 1 or > 64 || request.Packages is null
            || request.Packages.Length is < 1 or > 4096 || request.Textures is < 1 or > 100_000)
            throw new InvalidDataException("Invalid M3TO compilation request.");
        foreach (string root in new[] { request.GameRoot, request.InputRoot, request.OutputRoot })
            TransformProtocol.ValidateRoot(root);
        if (TransformProtocol.Overlaps(request.GameRoot, request.OutputRoot)
            || TransformProtocol.Overlaps(request.InputRoot, request.OutputRoot)
            || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any())
            throw new InvalidDataException("M3TO output must be empty and separate from inputs.");

        string prefix = $"DLC/{request.Dlc}/CookedPCConsole/";
        var paths = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long total = 0;
        foreach (var manifest in request.Manifests)
        {
            string path = Identity(manifest, ManifestLimit, ref total);
            string name = Path.GetFileName(path);
            if (!path.StartsWith(prefix, StringComparison.Ordinal) || path.Split('/').Length != 4
                || !name.StartsWith("TextureOverride-", StringComparison.Ordinal)
                || name.Length <= 21 || !name.EndsWith(".m3to", StringComparison.OrdinalIgnoreCase)
                || !paths.Add(path))
                throw new InvalidDataException("Invalid or duplicate M3TO manifest input.");
        }
        foreach (var package in request.Packages)
        {
            string path = Identity(package, PackageLimit, ref total);
            string name = Path.GetFileName(path);
            if (!path.StartsWith(prefix, StringComparison.Ordinal) || path.Split('/').Length != 4
                || !name.StartsWith("TO_", StringComparison.Ordinal) || name.Length <= 7
                || !name.EndsWith(".pcc", StringComparison.OrdinalIgnoreCase) || !paths.Add(path))
                throw new InvalidDataException("Invalid or duplicate M3TO package input.");
        }
        if (total > JobLimit)
            throw new InvalidDataException("M3TO inputs exceed 4 GiB.");
        string[] expected = { $"DLC/{request.Dlc}/CombinedTextureOverrides.btp", $"DLC/{request.Dlc}/BTPMetadata.btm" };
        if (request.Outputs is null || !request.Outputs.SequenceEqual(expected, StringComparer.Ordinal))
            throw new InvalidDataException("M3TO output inventory differs from its plan.");
    }

    private static string Identity(InputFile input, long limit, ref long total)
    {
        if (input is null || input.Size is < 1 || input.Size > limit)
            throw new InvalidDataException("Invalid M3TO input size.");
        total = checked(total + input.Size);
        return TransformProtocol.Relative(input.Path);
    }

    internal static OutputFile[] Execute(TextureRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        cancellation.ThrowIfCancellationRequested();
        if (!OodleHelper.EnsureOodleDll(request.GameRoot))
            throw new InvalidDataException("The verified game codec is unavailable.");
        var game = Enum.Parse<MEGame>(request.Game, ignoreCase: false);
        string scratch = Path.Combine(request.OutputRoot, ".m3to-inputs");
        Directory.CreateDirectory(scratch);
        try
        {
            var packageNames = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            foreach (var identity in request.Packages)
            {
                cancellation.ThrowIfCancellationRequested();
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, identity, PackageLimit);
                using (var package = MEPackageHandler.OpenMEPackageFromStream(bytes))
                    if (package.Game != game)
                        throw new InvalidDataException("An M3TO source package belongs to another game.");
                bytes.Position = 0;
                string name = Path.GetFileName(identity.Path);
                if (!packageNames.TryAdd(name, name))
                    throw new InvalidDataException("M3TO source package names are ambiguous.");
                using var destination = new FileStream(Path.Combine(scratch, name), FileMode.CreateNew,
                    FileAccess.Write, FileShare.None);
                bytes.CopyTo(destination);
                destination.Flush(flushToDisk: true);
            }

            var merged = new List<TextureOverrideTextureEntry>();
            foreach (var identity in request.Manifests)
            {
                cancellation.ThrowIfCancellationRequested();
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, identity, ManifestLimit);
                using var document = JsonDocument.Parse(bytes, new JsonDocumentOptions { MaxDepth = 8 });
                TransformProtocol.RejectDuplicateKeys(document.RootElement);
                var manifest = document.Deserialize<TextureManifestDocument>(TransformProtocol.Json)
                    ?? throw new InvalidDataException("An M3TO manifest is required.");
                if (manifest.Game != request.Game || manifest.Textures is null
                    || manifest.Textures.Length is < 1 or > 100_000)
                    throw new InvalidDataException("Invalid M3TO manifest game or texture count.");
                foreach (var item in manifest.Textures)
                {
                    if (item is null || !packageNames.TryGetValue(item.Sourcepackage, out string sourcePackage)
                        || item.Sourcepackage != Path.GetFileName(item.Sourcepackage)
                        || item.Textureifp is null || item.Textureifp.Length is < 3 or > 255
                        || !item.Textureifp.Contains('.') || item.Textureifp.Split('.').Any(part =>
                            part.Length == 0 || part.Any(character => !char.IsAsciiLetterOrDigit(character) && character != '_')))
                        throw new InvalidDataException("Invalid M3TO texture declaration.");
                    merged.RemoveAll(existing => existing.TextureIFP.Equals(item.Textureifp, StringComparison.OrdinalIgnoreCase));
                    merged.Add(new TextureOverrideTextureEntry
                    {
                        CompilingSourcePackage = sourcePackage,
                        TextureIFP = item.Textureifp,
                    });
                    if (merged.Count > 100_000)
                        throw new InvalidDataException("M3TO merged texture count exceeds its limit.");
                }
            }
            if (merged.Count != request.Textures)
                throw new InvalidDataException("M3TO merged texture count differs from its plan.");

            string btp = OutputPath(request, request.Outputs[0]);
            string btm = OutputPath(request, request.Outputs[1]);
            using (var metadata = MEPackageHandler.CreateAndOpenPackage(btm, game))
            {
                metadata.FindNameOrAdd(request.Dlc);
                using var stream = new FileStream(btp, FileMode.CreateNew, FileAccess.ReadWrite, FileShare.None);
                new TextureOverrideCompiler().Build(new TextureManifest { Game = game, Textures = merged },
                    scratch, stream, request.Dlc, progress, metadata);
                stream.Flush(flushToDisk: true);
            }
            cancellation.ThrowIfCancellationRequested();
            return request.Outputs.Select(path => Output(request.OutputRoot, path)).ToArray();
        }
        finally
        {
            if (Directory.Exists(scratch)) Directory.Delete(scratch, recursive: true);
        }
    }

    private static string OutputPath(TextureRequest request, string relative)
    {
        string path = Path.Combine(request.OutputRoot, TransformProtocol.Relative(relative));
        Directory.CreateDirectory(Path.GetDirectoryName(path)
            ?? throw new InvalidDataException("M3TO output has no parent directory."));
        return path;
    }

    private static OutputFile Output(string root, string relative)
    {
        string path = Path.Combine(root, relative);
        var info = new FileInfo(path);
        if (!info.Exists || info.Length is < 1 or > 4L * 1024 * 1024 * 1024)
            throw new InvalidDataException("Invalid M3TO compiler output.");
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        return new OutputFile(relative, info.Length, Convert.ToHexStringLower(SHA256.HashData(stream)));
    }
}
