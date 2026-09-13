using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
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

internal sealed record PlotContribution(string Dlc, int Mount, InputFile Manifest);
internal sealed record PlotRequest(int Protocol, string Operation, string GameRoot, string OriginalRoot,
    string InputRoot, string OutputRoot, TargetPackage Target, InputFile[] Dependencies, PlotContribution[] Contributions, string Game = "LE1");

internal static class PlotMerge
{
    private static MEGame Game(PlotRequest request) => request.Game switch
    {
        "LE1" => MEGame.LE1,
        "LE2" => MEGame.LE2,
        _ => throw new InvalidDataException("Plot updates require LE1 or LE2."),
    };

    internal const string Target = "CookedPCConsole/PlotManager.pcc";

    internal static PlotRequest ReadRequest(string path)
    {
        var request = TransformProtocol.ReadJson<PlotRequest>(path);
        Validate(request);
        return request;
    }

    internal static void Validate(PlotRequest request)
    {
        var game = Game(request);
        if (request.Protocol != 1 || request.Operation is not ("le1-plot" or "mele-plot") || request.Target?.Original is null || request.Target.Current is null
            || request.Target.Original.Path != Target || request.Target.Current.Path != Target
            || request.Dependencies is null || request.Dependencies.Length > 8
            || request.Contributions is null || request.Contributions.Length > 1024)
            throw new InvalidDataException("Invalid plot merge request.");
        foreach (string root in new[] { request.GameRoot, request.OriginalRoot, request.InputRoot, request.OutputRoot }) TransformProtocol.ValidateRoot(root);
        if (new[] { request.GameRoot, request.OriginalRoot, request.InputRoot }.Any(root => TransformProtocol.Overlaps(root, request.OutputRoot))
            || TransformProtocol.Overlaps(request.OriginalRoot, request.InputRoot) || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any())
            throw new InvalidDataException("Plot output must be empty and separate from inputs; originals and candidates must be separate.");
        var mounts = new Dictionary<int, string>();
        var dlcs = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var manifests = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var item in request.Contributions)
        {
            if (item?.Dlc is null || !item.Dlc.StartsWith("DLC_MOD_", StringComparison.OrdinalIgnoreCase)
                || item.Dlc.Length > 255 || item.Dlc.Any(ch => !char.IsAsciiLetterOrDigit(ch) && ch != '_')
                || (dlcs.TryGetValue(item.Dlc, out int priority) && priority != item.Mount)
                || (mounts.TryGetValue(item.Mount, out string owner) && !owner.Equals(item.Dlc, StringComparison.OrdinalIgnoreCase))
                || item.Manifest?.Path is null || !manifests.Add(item.Manifest.Path)
                || !item.Manifest.Path.StartsWith($"DLC/{item.Dlc}/CookedPCConsole/", StringComparison.Ordinal)
                || item.Manifest.Path.Split('/').Length != 4 || !item.Manifest.Path.EndsWith(".pmu", StringComparison.Ordinal)
                || item.Manifest.Size is < 1 or > 1024 * 1024)
                throw new InvalidDataException("Invalid plot contribution or ambiguous DLC mount order.");
            TransformProtocol.Relative(item.Manifest.Path);
            dlcs[item.Dlc] = item.Mount;
            mounts[item.Mount] = item.Dlc;
        }
        var dependencies = request.Contributions.Length == 0 ? Array.Empty<string>()
            : FileLib.BaseFileNames(game).Select(name => "CookedPCConsole/" + name).ToArray();
        if (request.Dependencies.Any(input => input is null)
            || request.Dependencies.Select(input => input.Path).Distinct(StringComparer.OrdinalIgnoreCase).Count() != request.Dependencies.Length
            || !request.Dependencies.Select(input => input.Path).ToHashSet(StringComparer.Ordinal).SetEquals(dependencies))
            throw new InvalidDataException("Plot compilation requires exactly the declared game base packages.");
        long total = 0;
        foreach (var input in request.Dependencies.Concat(new[] { request.Target.Original, request.Target.Current }).Concat(request.Contributions.Select(item => item.Manifest)))
        {
            if (input.Size is < 1 or > 512 * 1024 * 1024) throw new InvalidDataException("Invalid plot input size.");
            total += input.Size;
        }
        if (total > 2L * 1024 * 1024 * 1024) throw new InvalidDataException("Plot inputs exceed the request size limit.");
    }

    internal static KeyValuePair<int, string>[] Parse(string text)
    {
        if (text.Length > 1024 * 1024 || text.Contains('\0')) throw new InvalidDataException("Invalid plot source text.");
        var functions = new List<KeyValuePair<int, string>>();
        StringBuilder source = null;
        int id = 0;
        using var reader = new StringReader(text.TrimStart('\uFEFF'));
        string line;
        while ((line = reader.ReadLine()) is not null)
        {
            const string prefix = "public function bool F";
            if (line.StartsWith(prefix, StringComparison.Ordinal))
            {
                if (source is not null) functions.Add(new(id, source.ToString()));
                int end = line.IndexOf('(', prefix.Length);
                string number = end < 0 ? "" : line[prefix.Length..end];
                if (!int.TryParse(number, out id) || id <= 0 || id.ToString(System.Globalization.CultureInfo.InvariantCulture) != number)
                    throw new InvalidDataException("Plot IDs must be positive integers without leading zeros.");
                source = new StringBuilder();
            }
            if (source is not null) source.Append(line).Append('\n');
            else if (!string.IsNullOrWhiteSpace(line) && !line.TrimStart().StartsWith("//", StringComparison.Ordinal))
                throw new InvalidDataException("Unexpected text before plot conditional.");
            if (functions.Count >= 4096) throw new InvalidDataException("Too many plot conditionals.");
        }
        if (source is not null) functions.Add(new(id, source.ToString()));
        if (functions.Count == 0) throw new InvalidDataException("Plot update contains no conditionals.");
        return functions.ToArray();
    }

    internal static void AddMissing(IMEPackage package, IEnumerable<int> ids)
    {
        var parent = package.FindExport("BioAutoConditionals");
        var template = package.Exports.FirstOrDefault(export => export.ClassName == "Function" && export.Parent == parent);
        if (package.Game is not (MEGame.LE1 or MEGame.LE2) || parent is null || !parent.IsClass || template is null)
            throw new InvalidDataException("Original PlotManager lacks its conditional class or function template.");
        foreach (int id in ids)
        {
            string name = "F" + id.ToString(System.Globalization.CultureInfo.InvariantCulture);
            if (package.FindExport("BioAutoConditionals." + name) is not null) continue;
            var added = EntryCloner.CloneTree(template);
            added.ObjectName = new NameReference(name, 0);
            package.InvalidateLookupTable();
            var binary = ObjectBinary.From<UFunction>(added);
            binary.ScriptBytes = Array.Empty<byte>();
            added.WriteBinary(binary);
        }
        var conditionalClass = ObjectBinary.From<UClass>(parent);
        conditionalClass.UpdateChildrenChain();
        conditionalClass.UpdateLocalFunctions();
        parent.WriteBinary(conditionalClass);
    }

    internal static OutputFile[] Execute(PlotRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        var game = Game(request);
        cancellation.ThrowIfCancellationRequested();
        using var original = TransformProtocol.ReadVerified(request.OriginalRoot, request.Target.Original);
        using var current = TransformProtocol.ReadVerified(request.InputRoot, request.Target.Current);
        var functions = new Dictionary<int, string>();
        foreach (var item in request.Contributions.OrderBy(item => item.Mount))
        {
            cancellation.ThrowIfCancellationRequested();
            using var data = TransformProtocol.ReadVerified(request.InputRoot, item.Manifest, 1024 * 1024);
            foreach (var function in Parse(new UTF8Encoding(false, true).GetString(data.ToArray()))) functions[function.Key] = function.Value;
            if (functions.Count > 4096) throw new InvalidDataException("Combined plot updates exceed the conditional limit.");
        }
        if (!OodleHelper.EnsureOodleDll(request.GameRoot)) throw new InvalidDataException("The verified game codec is unavailable.");
        if (functions.Count == 0)
        {
            cancellation.ThrowIfCancellationRequested();
            using var baseline = MEPackageHandler.OpenMEPackageFromStream(original, "PlotManager.pcc");
            if (baseline.Game != game || baseline.FindExport("BioAutoConditionals") is not { IsClass: true })
                throw new InvalidDataException("Original PlotManager is not a matching conditional package.");
            original.Position = 0;
            cancellation.ThrowIfCancellationRequested();
            string destination = Path.Combine(request.OutputRoot, Target);
            Directory.CreateDirectory(Path.GetDirectoryName(destination));
            TransformProtocol.ValidateRoot(Path.GetDirectoryName(destination));
            using var file = new FileStream(destination, FileMode.CreateNew, FileAccess.Write, FileShare.None);
            original.CopyTo(file);
            file.Flush(flushToDisk: true);
            return new[] { new OutputFile(Target, request.Target.Original.Size, request.Target.Original.Sha256.ToLowerInvariant()) };
        }
        if (game == MEGame.LE1) LE1UnrealObjectInfo.ObjectInfo.LoadData(null);
        else LE2UnrealObjectInfo.ObjectInfo.LoadData(null);
        string resolution = Path.Combine(request.OutputRoot, ".resolution");
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
                cancellation.ThrowIfCancellationRequested();
                using var data = TransformProtocol.ReadVerified(request.InputRoot, input);
                var dependency = MEPackageHandler.OpenMEPackageFromStream(data, Path.Combine(resolution, "BioGame", input.Path));
                packages.Add(Path.GetFileName(input.Path), dependency);
                if (dependency.Game != game) throw new InvalidDataException("Plot dependency is not a matching game package.");
            }
            using var target = MEPackageHandler.OpenMEPackageFromStream(original, Path.Combine(resolution, "BioGame", Target));
            if (target.Game != game) throw new InvalidDataException("Plot target does not match the requested game.");
            AddMissing(target, functions.Keys);
            using var cache = new ScriptPackageCache(packages, resolution);
            var options = new UnrealScriptOptionsPackage { Cache = cache, GamePathOverride = resolution,
                CustomFileResolver = (name, _) => cache.ResolveCandidate(name) };
            using var library = new FileLib(target, useAutoReinitialization: false);
            if (!library.Initialize(options, canUseBinaryCache: false))
                throw new InvalidDataException($"Plot compiler initialization failed: {library.InitializationLog}");
            int completed = 0;
            foreach (var function in functions)
            {
                cancellation.ThrowIfCancellationRequested();
                M3mScripts.Apply(target, new ScriptMerge("BioAutoConditionals.F" + function.Key, "function", ""), function.Value, library, options);
                progress(++completed, functions.Count);
            }
            return new[] { PackageOutput.Write(target, request.OutputRoot, Target, cancellation) };
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
