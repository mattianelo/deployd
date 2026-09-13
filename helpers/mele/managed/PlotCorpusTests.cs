using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;

using LegendaryExplorerCore;
using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.GameFilesystem;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;
using LegendaryExplorerCore.Unreal.ObjectInfo;
using LegendaryExplorerCore.UnrealScript;

namespace Deployd.Mele;

internal static class PlotCorpusTests
{
    internal static void Verify(string root)
    {
        var request = PlotMerge.ReadRequest(Path.Combine(root, "semantic-request.json"));
        var combined = PlotMerge.ReadRequest(Path.Combine(root, "combined-request.json"));
        TlkPlotTests.Require(OodleHelper.EnsureOodleDll(request.GameRoot), "Verified codec unavailable.");
        if (request.Game == "LE1") LE1UnrealObjectInfo.ObjectInfo.LoadData(null);
        else LE2UnrealObjectInfo.ObjectInfo.LoadData(null);
        Reference(request, Path.Combine(root, "merged"));
        Reference(combined, Path.Combine(root, "combined"));
        using (var first = MEPackageHandler.OpenMEPackage(Path.Combine(root, "merged", PlotMerge.Target), forceLoadFromDisk: true))
        using (var again = MEPackageHandler.OpenMEPackage(Path.Combine(root, "reapplied", PlotMerge.Target), forceLoadFromDisk: true))
        using (var original = MEPackageHandler.OpenMEPackage(Path.Combine(request.OriginalRoot, PlotMerge.Target), forceLoadFromDisk: true))
        {
            ScriptMergeTests.SamePackage(first, again);
            var ids = request.Contributions.SelectMany(item => PlotMerge.Parse(File.ReadAllText(Path.Combine(request.InputRoot, item.Manifest.Path))).Select(item => item.Key)).ToHashSet();
            foreach (var entry in original.Exports)
            {
                bool changed = entry.InstancedFullPath == "BioAutoConditionals" || ids.Any(id =>
                    entry.InstancedFullPath == $"BioAutoConditionals.F{id}" || entry.InstancedFullPath.StartsWith($"BioAutoConditionals.F{id}.", StringComparison.Ordinal));
                if (!changed)
                    TlkPlotTests.Require(entry.Data.AsSpan().SequenceEqual(first.FindExport(entry.InstancedFullPath).Data), "Unrelated PlotManager export changed.");
            }
        }
        using var cancellation = new CancellationTokenSource();
        TlkPlotTests.Cancelled(() => PlotMerge.Execute(request, cancellation.Token, (done, _) => { if (done == 1) cancellation.Cancel(); }));
        TlkPlotTests.Require(!Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "Cancelled plot merge published output.");
        bool progressed = false;
        TlkPlotTests.Reject(() => PlotMerge.Execute(request with { Target = request.Target with { Original = request.Target.Original with { Sha256 = new string('0', 64) } } },
            CancellationToken.None, (_, _) => progressed = true));
        TlkPlotTests.Require(!progressed && !Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "Changed original reached plot compilation.");
        TlkPlotTests.Reject(() => PlotMerge.Validate(combined with { Contributions = combined.Contributions.Select(item => item with { Mount = 5 }).ToArray() }));
        TlkPlotTests.Reject(() => PlotMerge.Validate(request with { Dependencies = request.Dependencies[..^1] }));
        Console.WriteLine("Plot corpus passed conditional updates, mount overrides, reference, rebuild, cancellation, and dependency validation.");
    }

    private static void Reference(PlotRequest request, string actualRoot)
    {
        string resolution = Path.Combine(request.OutputRoot, "reference");
        Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "CookedPCConsole"));
        Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "DLC"));
        var packages = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        string previous = LE1Directory.DefaultGamePath;
        string previousLe2 = LE2Directory.DefaultGamePath;
        string previousLe3 = LE3Directory.DefaultGamePath;
        string family = LegendaryExplorerCoreLibSettings.Instance.LEDirectory;
        LE1Directory.DefaultGamePath = null;
        LE2Directory.DefaultGamePath = null;
        LE3Directory.DefaultGamePath = null;
        LegendaryExplorerCoreLibSettings.Instance.LEDirectory = null;
        try
        {
            foreach (var input in request.Dependencies)
            {
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
                packages.Add(Path.GetFileName(input.Path), MEPackageHandler.OpenMEPackageFromStream(bytes, Path.Combine(resolution, "BioGame", input.Path)));
            }
            using var bytesOriginal = TransformProtocol.ReadVerified(request.OriginalRoot, request.Target.Original);
            using var expected = MEPackageHandler.OpenMEPackageFromStream(bytesOriginal, Path.Combine(resolution, "BioGame", PlotMerge.Target));
            var functions = new Dictionary<int, string>();
            foreach (var item in request.Contributions.OrderBy(item => item.Mount))
                foreach (var function in PlotMerge.Parse(File.ReadAllText(Path.Combine(request.InputRoot, item.Manifest.Path)))) functions[function.Key] = function.Value;
            var template = expected.Exports.First(export => export.ClassName == "Function");
            foreach (int id in functions.Keys)
            {
                if (expected.FindExport($"BioAutoConditionals.F{id}") is not null) continue;
                var added = EntryCloner.CloneTree(template);
                added.ObjectName = new NameReference($"F{id}", 0);
                expected.InvalidateLookupTable();
                var binary = ObjectBinary.From<UFunction>(added);
                binary.ScriptBytes = Array.Empty<byte>();
                added.WriteBinary(binary);
            }
            var conditionalClass = ObjectBinary.From<UClass>(expected.FindExport("BioAutoConditionals"));
            conditionalClass.UpdateChildrenChain();
            conditionalClass.UpdateLocalFunctions();
            conditionalClass.Export.WriteBinary(conditionalClass);
            using var cache = new ScriptPackageCache(packages, resolution);
            var options = new UnrealScriptOptionsPackage { Cache = cache, GamePathOverride = resolution,
                CustomFileResolver = (name, _) => cache.ResolveCandidate(name) };
            using var library = new FileLib(expected);
            TlkPlotTests.Require(library.Initialize(options, canUseBinaryCache: false), "Pinned plot reference symbols unavailable.");
            foreach (var function in functions)
            {
                var (_, log) = UnrealScriptCompiler.CompileFunction(expected.FindExport($"BioAutoConditionals.F{function.Key}"), function.Value, library, options);
                TlkPlotTests.Require(!log.HasErrors && !log.HasLexErrors, $"Pinned plot reference failed to compile F{function.Key}: {log}");
                TlkPlotTests.Require(library.ReInitializeFile(options), "Pinned plot reference symbols could not be refreshed.");
            }
            using var serialized = expected.SaveToStream(compress: true);
            using var actual = MEPackageHandler.OpenMEPackage(Path.Combine(actualRoot, PlotMerge.Target), forceLoadFromDisk: true);
            ScriptMergeTests.SamePackage(expected, actual);
        }
        finally
        {
            LE1Directory.DefaultGamePath = previous;
            LE2Directory.DefaultGamePath = previousLe2;
            LE3Directory.DefaultGamePath = previousLe3;
            LegendaryExplorerCoreLibSettings.Instance.LEDirectory = family;
            foreach (var package in packages.Values) package.Dispose();
            Directory.Delete(resolution, recursive: true);
        }
    }
}
