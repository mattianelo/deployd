using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.TLK;
using LegendaryExplorerCore.TLK.ME1;

namespace Deployd.Mele;

internal sealed record TlkString(int Id, string Data);
internal sealed record TlkChange(string Target, string Export, TlkString[] Strings);
internal sealed record TlkRequest(int Protocol, string Operation, string GameRoot, string InputRoot,
    string OutputRoot, InputFile[] Targets, TlkChange[] Changes);

internal static class TlkMerge
{
    internal static TlkRequest ReadRequest(string path)
    {
        var request = TransformProtocol.ReadJson<TlkRequest>(path);
        Validate(request);
        return request;
    }

    internal static bool IsTarget(string path)
    {
        string[] parts = TransformProtocol.Relative(path).Split('/');
        bool Identifier(string text) => text.Length is > 0 and <= 255
            && text.All(ch => char.IsAsciiLetterOrDigit(ch) || ch == '_');
        return ((parts.Length >= 2 && parts[0] == "CookedPCConsole")
            || (parts.Length >= 4 && parts[0] == "DLC" && Identifier(parts[1]) && parts[2] == "CookedPCConsole"))
            && parts[^1].EndsWith(".pcc", StringComparison.Ordinal) && Identifier(parts[^1][..^4]);
    }

    internal static void Validate(TlkRequest request)
    {
        if (request.Protocol != 1 || request.Operation != "le1-tlk" || request.Targets is null
            || request.Targets.Length is < 1 or > 64 || request.Changes is null || request.Changes.Length is < 1 or > 4096)
            throw new InvalidDataException("Invalid embedded TLK request.");
        foreach (string root in new[] { request.GameRoot, request.InputRoot, request.OutputRoot }) TransformProtocol.ValidateRoot(root);
        if (TransformProtocol.Overlaps(request.GameRoot, request.OutputRoot) || TransformProtocol.Overlaps(request.InputRoot, request.OutputRoot)
            || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any())
            throw new InvalidDataException("TLK output must be empty and separate from inputs.");
        var targets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long total = 0;
        foreach (var target in request.Targets)
        {
            if (target is null || !IsTarget(target.Path) || !targets.Add(target.Path) || target.Size is < 1 or > 512 * 1024 * 1024)
                throw new InvalidDataException("Invalid or duplicate TLK target.");
            total += target.Size;
        }
        if (total > 2L * 1024 * 1024 * 1024) throw new InvalidDataException("TLK targets exceed the request size limit.");
        var used = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long textSize = 0;
        foreach (var change in request.Changes)
        {
            if (change is null || !targets.Contains(change.Target) || change.Strings is null || change.Strings.Length > 16384)
                throw new InvalidDataException("Invalid TLK update or undeclared target.");
            M3mAssets.EntryName(change.Export);
            used.Add(change.Target);
            var ids = new HashSet<int>();
            foreach (var value in change.Strings)
            {
                if (value is null || value.Data is null || value.Data.Contains('\0') || !ids.Add(value.Id))
                    throw new InvalidDataException("Duplicate TLK ID or invalid string data.");
                textSize += value.Data.Length;
            }
        }
        if (!used.SetEquals(targets) || textSize > 4 * 1024 * 1024)
            throw new InvalidDataException("TLK request has unused targets or excessive string data.");
    }

    internal static void Apply(IMEPackage package, TlkChange change)
    {
        var export = package.FindExport(change.Export);
        if (package.Game != MEGame.LE1 || export is null || export.ClassName != "BioTlkFile" || export.IsDefaultObject)
            throw new InvalidDataException("TLK target is missing or is not an LE1 talk-file export.");
        if (change.Strings.Length == 0) return;
        var references = new ME1TalkFile(export).StringRefs;
        foreach (var value in change.Strings)
        {
            var existing = references.FirstOrDefault(entry => entry.StringID == value.Id);
            if (existing is null) references.Add(new TLKStringRef(value.Id, value.Data));
            else existing.Data = value.Data;
        }
        var compression = new HuffmanCompression();
        compression.LoadInputData(references);
        compression.SerializeTalkfileToExport(export);
    }

    internal static OutputFile[] Execute(TlkRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        cancellation.ThrowIfCancellationRequested();
        if (!OodleHelper.EnsureOodleDll(request.GameRoot)) throw new InvalidDataException("The verified game codec is unavailable.");
        var packages = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        try
        {
            foreach (var input in request.Targets)
            {
                cancellation.ThrowIfCancellationRequested();
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
                var package = MEPackageHandler.OpenMEPackageFromStream(bytes, Path.GetFileName(input.Path));
                packages.Add(input.Path, package);
                if (package.Game != MEGame.LE1) throw new InvalidDataException("TLK input is not an LE1 package.");
            }
            for (int index = 0; index < request.Changes.Length; index++)
            {
                cancellation.ThrowIfCancellationRequested();
                var change = request.Changes[index];
                Apply(packages[change.Target], change);
                progress(index + 1, request.Changes.Length);
            }
            return request.Targets.Select(target => PackageOutput.Write(packages[target.Path], request.OutputRoot, target.Path, cancellation)).ToArray();
        }
        finally { foreach (var package in packages.Values) package.Dispose(); }
    }
}
