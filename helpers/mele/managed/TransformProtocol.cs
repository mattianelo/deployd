using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Deployd.Mele;

internal sealed record InputFile(string Path, long Size, string Sha256);
internal sealed record TargetPackage(InputFile Original, InputFile Current);
internal sealed record Contribution(string Dlc, int Mount, InputFile Manifest, InputFile[] Packages);
internal sealed record MergeRequest(int Protocol, string Operation, string GameRoot, string OriginalRoot,
    string InputRoot, string OutputRoot, TargetPackage[] Targets, Contribution[] Contributions);
internal sealed record OutputFile(string Path, long Size, string Sha256);

internal static class TransformProtocol
{
    internal static T ReadJson<T>(string path) where T : class
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (stream.Length > 4 * 1024 * 1024) throw new InvalidDataException("Merge request exceeds its size limit.");
        using var document = JsonDocument.Parse(stream, new JsonDocumentOptions { MaxDepth = 16 });
        RejectDuplicateKeys(document.RootElement);
        return document.Deserialize<T>(Json) ?? throw new InvalidDataException("A merge request is required.");
    }
    internal static readonly JsonSerializerOptions Json = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow,
        RespectRequiredConstructorParameters = true,
    };

    internal static MergeRequest ReadRequest(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (stream.Length > 4 * 1024 * 1024)
            throw new InvalidDataException("Transformation request exceeds its size limit.");
        using var document = JsonDocument.Parse(stream, new JsonDocumentOptions { MaxDepth = 32 });
        RejectDuplicateKeys(document.RootElement);
        var request = document.Deserialize<MergeRequest>(Json)
            ?? throw new InvalidDataException("A transformation request is required.");
        Validate(request);
        return request;
    }

    internal static void RejectDuplicateKeys(JsonElement element)
    {
        if (element.ValueKind == JsonValueKind.Object)
        {
            var keys = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var property in element.EnumerateObject())
            {
                if (!keys.Add(property.Name))
                    throw new InvalidDataException("Duplicate JSON property.");
                RejectDuplicateKeys(property.Value);
            }
        }
        else if (element.ValueKind == JsonValueKind.Array)
            foreach (var item in element.EnumerateArray())
                RejectDuplicateKeys(item);
    }

    internal static void Validate(MergeRequest request)
    {
        if (request.Protocol != 1 || request.Operation is not ("le1-m3da" or "le1-m3cd"))
            throw new InvalidDataException("Unsupported transformation protocol or operation.");
        bool config = request.Operation == "le1-m3cd";
        if (request.Targets is null || request.Targets.Length is < 1 or > 11
            || request.Contributions is null || request.Contributions.Length > 4096)
            throw new InvalidDataException("Invalid transformation input count.");
        var roots = new[] { request.GameRoot, request.OriginalRoot, request.InputRoot, request.OutputRoot };
        foreach (string root in roots)
            ValidateRoot(root);
        long totalBytes = 0;
        void Count(InputFile input)
        {
            if (input is null || input.Size < 0 || input.Size > 512 * 1024 * 1024)
                throw new InvalidDataException("Invalid transformation input size.");
            totalBytes = checked(totalBytes + input.Size);
            if (totalBytes > 4L * 1024 * 1024 * 1024)
                throw new InvalidDataException("Transformation inputs exceed the job size limit.");
        }
        for (int index = 0; index < roots.Length - 1; index++)
            if (Overlaps(roots[index], request.OutputRoot))
                throw new InvalidDataException("Output must be separate from every input and the game.");
        if (Overlaps(request.OriginalRoot, request.InputRoot))
            throw new InvalidDataException("Original and candidate inputs must be separate.");
        if (Directory.EnumerateFileSystemEntries(request.OutputRoot).Any())
            throw new InvalidDataException("Output directory must be empty.");
        var targets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var target in request.Targets)
        {
            if (target?.Original is null || target.Current is null)
                throw new InvalidDataException("Missing target identity.");
            Count(target.Original);
            Count(target.Current);
            string original = Relative(target.Original.Path);
            string current = Relative(target.Current.Path);
            if (!string.Equals(original, current, StringComparison.OrdinalIgnoreCase)
                || original.Split('/').Length != 2 || !original.StartsWith("CookedPCConsole/", StringComparison.OrdinalIgnoreCase)
                || !(config ? M3cdMerge.IsTarget(Path.GetFileName(original))
                    : M3daMerge.AllowedTargets.Contains(Path.GetFileName(original), StringComparer.OrdinalIgnoreCase))
                || !targets.Add(current))
                throw new InvalidDataException("Invalid or duplicate transformation target.");
        }
        var manifests = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var mounts = new Dictionary<int, string>();
        var dlcs = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        foreach (var contribution in request.Contributions)
        {
            if (contribution is null || contribution.Manifest is null || contribution.Packages is null
                || (config ? contribution.Packages.Length != 0 : contribution.Packages.Length is < 1 or > 1024))
                throw new InvalidDataException("Missing or unsupported merge contribution inputs.");
            string dlc = Relative(contribution.Dlc);
            if (dlc.Contains('/') || !dlc.StartsWith("DLC_MOD_", StringComparison.OrdinalIgnoreCase))
                throw new InvalidDataException("LE1 merges require a custom DLC identity.");
            if ((mounts.TryGetValue(contribution.Mount, out string owner) && !string.Equals(owner, dlc, StringComparison.OrdinalIgnoreCase))
                || (dlcs.TryGetValue(dlc, out int mount) && mount != contribution.Mount))
                throw new InvalidDataException("Ambiguous DLC mount ordering.");
            mounts[contribution.Mount] = dlc;
            dlcs[dlc] = contribution.Mount;
            string manifest = Relative(contribution.Manifest.Path);
            Count(contribution.Manifest);
            string prefix = $"DLC/{dlc}/CookedPCConsole/";
            string filename = Path.GetFileName(manifest);
            if (!manifest.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)
                || (config ? manifest.Split('/').Length != 4 || !M3cdMerge.IsManifest(filename)
                    : !filename.StartsWith(dlc + "-", StringComparison.OrdinalIgnoreCase)
                        || !filename.EndsWith(".m3da", StringComparison.OrdinalIgnoreCase)
                        || filename.Length <= dlc.Length + 6)
                || !manifests.Add(manifest))
                throw new InvalidDataException("Invalid or duplicate merge manifest location.");
            var packages = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var package in contribution.Packages)
            {
                Count(package);
                if (package is null || !Relative(package.Path).StartsWith(prefix, StringComparison.OrdinalIgnoreCase)
                    || !package.Path.EndsWith(".pcc", StringComparison.OrdinalIgnoreCase)
                    || !packages.Add(Path.GetFileName(package.Path)))
                    throw new InvalidDataException("Invalid or ambiguous M3DA source package.");
            }
        }
    }

    internal static bool Overlaps(string first, string second)
    {
        first = Path.TrimEndingDirectorySeparator(Path.GetFullPath(first));
        second = Path.TrimEndingDirectorySeparator(Path.GetFullPath(second));
        return first == second || first.StartsWith(second + "/", StringComparison.Ordinal)
            || second.StartsWith(first + "/", StringComparison.Ordinal);
    }

    internal static void ValidateRoot(string root)
    {
        if (string.IsNullOrWhiteSpace(root) || !Path.IsPathFullyQualified(root)
            || !Directory.Exists(root) || Path.GetFullPath(root) == "/"
            || root.Split('/').Any(part => part is "." or ".."))
            throw new InvalidDataException("An explicit existing directory is required.");
        RejectLinks(root);
    }

    internal static string Relative(string path)
    {
        if (string.IsNullOrWhiteSpace(path) || path.Contains('\\') || path.Contains(':')
            || path.Any(char.IsControl) || path.Split('/').Any(part => part is "" or "." or ".." || part.EndsWith('.') || part.EndsWith(' ')))
            throw new InvalidDataException("Invalid relative transformation path.");
        return path;
    }

    private static void RejectLinks(string path)
    {
        for (string current = Path.GetFullPath(path); current is not null; current = Path.GetDirectoryName(current))
            if ((File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
                throw new InvalidDataException("Symbolic links are not allowed in transformation inputs.");
    }

    internal static MemoryStream ReadVerified(string root, InputFile input, long limit = 512 * 1024 * 1024)
    {
        if (input.Size < 0 || input.Size > limit || input.Sha256 is null || input.Sha256.Length != 64
            || input.Sha256.Any(character => !char.IsAsciiHexDigit(character)))
            throw new InvalidDataException("Invalid transformation input identity.");
        string path = Path.Combine(root, Relative(input.Path));
        RejectLinks(path);
        using var source = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (source.Length != input.Size)
            throw new InvalidDataException("Transformation input size changed.");
        var data = new byte[checked((int)input.Size)];
        source.ReadExactly(data);
        if (source.ReadByte() != -1 || !string.Equals(Convert.ToHexStringLower(SHA256.HashData(data)), input.Sha256, StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException("Transformation input hash changed.");
        return new MemoryStream(data, writable: false);
    }
}
