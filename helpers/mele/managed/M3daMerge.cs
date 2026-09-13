using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.Classes;

namespace Deployd.Mele;

internal sealed record M3daEntry(
    [property: JsonPropertyName("packagefile")] string Package,
    [property: JsonPropertyName("mergepackagefile")] string Source,
    [property: JsonPropertyName("mergetables")] string[] Tables,
    [property: JsonPropertyName("comment")] string Comment = null);

internal static class M3daMerge
{
    internal static readonly string[] AllowedTargets =
    {
        "Engine.pcc", "SFXGame.pcc", "EntryMenu.pcc", "BIOG_2DA_UNC_AreaMap_X.pcc",
        "BIOG_2DA_UNC_GalaxyMap_X.pcc", "BIOG_2DA_UNC_GamerProfile_X.pcc", "BIOG_2DA_UNC_Movement_X.pcc",
        "BIOG_2DA_UNC_Music_X.pcc", "BIOG_2DA_UNC_Talents_X.pcc", "BIOG_2DA_UNC_TreasureTables_X.pcc", "BIOG_2DA_UNC_UI_X.pcc",
    };

    internal static M3daEntry[] Parse(Stream stream)
    {
        using var document = JsonDocument.Parse(stream, new JsonDocumentOptions { AllowTrailingCommas = true, MaxDepth = 16 });
        TransformProtocol.RejectDuplicateKeys(document.RootElement);
        var entries = document.Deserialize<M3daEntry[]>(new JsonSerializerOptions(TransformProtocol.Json) { AllowTrailingCommas = true });
        if (entries is null || entries.Length is < 1 or > 1024)
            throw new InvalidDataException("Invalid M3DA entry count.");
        foreach (var entry in entries)
        {
            if (entry is null || !AllowedTargets.Contains(entry.Package, StringComparer.OrdinalIgnoreCase)
                || TransformProtocol.Relative(entry.Source).Contains('/') || !entry.Source.EndsWith(".pcc", StringComparison.OrdinalIgnoreCase)
                || entry.Tables is null || entry.Tables.Length is < 1 or > 4096)
                throw new InvalidDataException("Invalid M3DA package or table list.");
            var tables = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (string table in entry.Tables)
            {
                if (string.IsNullOrWhiteSpace(table) || table.Length > 1024 || table.Any(char.IsControl)
                    || table.Split('.').Any(string.IsNullOrEmpty) || !tables.Add(table))
                    throw new InvalidDataException("Invalid or duplicate M3DA table.");
                BaseName(table);
            }
        }
        return entries;
    }

    private static string BaseName(string path)
    {
        var name = NameReference.FromInstancedString(path.Split('.').Last());
        if (!name.Name.EndsWith("_part", StringComparison.Ordinal) || name.Name.Length <= 5 || name.Number < 0)
            throw new InvalidDataException("M3DA table names must end in _part with an optional instance number.");
        return name.Name[..^5];
    }

    private static bool IsTable(ExportEntry entry) => !entry.IsDefaultObject
        && entry.ClassName is "Bio2DA" or "Bio2DANumberedRows";

    internal static HashSet<string> ResetTables(IMEPackage original, IMEPackage current)
    {
        if (original.Game != MEGame.LE1 || current.Game != MEGame.LE1)
            throw new InvalidDataException("M3DA only supports LE1 packages.");
        var tables = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var source in original.Exports.Where(IsTable))
        {
            var destination = current.FindExport(source.InstancedFullPath);
            if (destination is null || destination.ClassName != source.ClassName)
                throw new InvalidDataException("An original table is missing or changed class in the candidate package.");
            var errors = EntryImporter.ImportAndRelinkEntries(EntryImporter.PortingOption.ReplaceSingularWithRelink,
                source, current, destination, true, new RelinkerOptionsPackage(), out _);
            if (errors.Count != 0)
                throw new InvalidDataException("Original table restoration could not relink all references.");
            tables.Add(destination.InstancedFullPath);
        }
        return tables;
    }

    internal static void Apply(IMEPackage source, IMEPackage target, string table, HashSet<string> originalTables)
    {
        string name = BaseName(table);
        var from = source.FindExport(table);
        if (source.Game != MEGame.LE1 || target.Game != MEGame.LE1 || from is null || !IsTable(from))
            throw new InvalidDataException("M3DA source is not an LE1 table export.");
        // Upstream also targets Bring Down the Sky's original _part tables.
        var candidates = target.Exports.Where(entry => IsTable(entry) &&
            (string.Equals(entry.ObjectName.Instanced, name, StringComparison.OrdinalIgnoreCase)
             || (entry.ObjectName.Name.StartsWith(name, StringComparison.OrdinalIgnoreCase)
                 && entry.ObjectName.Name.EndsWith("_part", StringComparison.Ordinal)))).ToArray();
        if (candidates.Length != 1 || !originalTables.Contains(candidates[0].InstancedFullPath))
            throw new InvalidDataException("M3DA target table is missing, ambiguous, or absent from the original package.");
        var destination = new Bio2DA(candidates[0]);
        var contribution = new Bio2DA(from);
        if (contribution.RowCount > 0 && destination.ColumnCount != 0 && !destination.ColumnNames.ToHashSet(StringComparer.OrdinalIgnoreCase)
            .SetEquals(contribution.ColumnNames))
            throw new InvalidDataException("M3DA column names differ from the target.");
        contribution.MergeInto(destination, out var result);
        if (result != Bio2DAMergeResult.OK)
            throw new InvalidDataException("M3DA rows could not be merged.");
        destination.Write2DAToExport();
    }

    internal static OutputFile[] Execute(MergeRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        TransformProtocol.Validate(request);
        if (request.Operation != "le1-m3da")
            throw new InvalidDataException("M3DA requires its own transformation operation.");
        if (!OodleHelper.EnsureOodleDll(request.GameRoot))
            throw new InvalidDataException("The verified game codec is unavailable.");
        var targets = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        var originals = new Dictionary<string, HashSet<string>>(StringComparer.OrdinalIgnoreCase);
        try
        {
            foreach (var pair in request.Targets)
            {
                cancellation.ThrowIfCancellationRequested();
                using var baselineBytes = TransformProtocol.ReadVerified(request.OriginalRoot, pair.Original);
                using var baseline = MEPackageHandler.OpenMEPackageFromStream(baselineBytes);
                using var candidateBytes = TransformProtocol.ReadVerified(request.InputRoot, pair.Current);
                var candidate = MEPackageHandler.OpenMEPackageFromStream(candidateBytes);
                string name = Path.GetFileName(pair.Current.Path);
                targets.Add(name, candidate);
                originals.Add(name, ResetTables(baseline, candidate));
            }
            var ordered = request.Contributions.OrderBy(item => item.Mount)
                .ThenBy(item => item.Manifest.Path, StringComparer.OrdinalIgnoreCase).ToArray();
            int completed = 0;
            foreach (var contribution in ordered)
            {
                cancellation.ThrowIfCancellationRequested();
                using var manifest = TransformProtocol.ReadVerified(request.InputRoot, contribution.Manifest, 1024 * 1024);
                foreach (var entry in Parse(manifest))
                {
                    if (!targets.TryGetValue(entry.Package, out var target))
                        throw new InvalidDataException("M3DA target was not included in the validated request.");
                    var identity = contribution.Packages.SingleOrDefault(file => string.Equals(
                        Path.GetFileName(file.Path), entry.Source, StringComparison.OrdinalIgnoreCase))
                        ?? throw new InvalidDataException("M3DA source package was not included in the validated request.");
                    using var sourceBytes = TransformProtocol.ReadVerified(request.InputRoot, identity);
                    using var source = MEPackageHandler.OpenMEPackageFromStream(sourceBytes);
                    foreach (string table in entry.Tables)
                    {
                        cancellation.ThrowIfCancellationRequested();
                        Apply(source, target, table, originals[entry.Package]);
                    }
                }
                progress(++completed, ordered.Length);
            }
            var outputs = new List<OutputFile>();
            foreach (var pair in request.Targets.OrderBy(item => item.Current.Path, StringComparer.Ordinal))
            {
                cancellation.ThrowIfCancellationRequested();
                var package = targets[Path.GetFileName(pair.Current.Path)];
                outputs.Add(PackageOutput.Write(package, request.OutputRoot, pair.Current.Path, cancellation));
            }
            return outputs.ToArray();
        }
        finally
        {
            foreach (var package in targets.Values)
                package.Dispose();
        }
    }
}
