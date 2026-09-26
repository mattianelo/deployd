using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;

namespace Deployd.Mele;

internal sealed record AssetMerge(string Target, string Entry, string Asset, string SourceEntry, bool AllowNew);
internal sealed record AssetMergeRequest(int Protocol, string Operation, string GameRoot, string InputRoot,
    string OutputRoot, InputFile[] Targets, InputFile[] Assets, AssetMerge[] Merges);

internal static class M3mAssets
{
    private static readonly string[] Le1Targets =
    {
        "Core.pcc", "Engine.pcc", "IpDrv.pcc", "GFxUI.pcc",
        "PlotManagerMap.pcc", "PlotManagerMap_LOC_INT.pcc", "SFXOnlineFoundation.pcc", "SFXGame.pcc",
        "SFXStrategicAI.pcc", "SFXGameContent_Powers.pcc", "PlotManager.pcc", "PlotManagerDLC_UNC.pcc",
        "BIOC_Materials.pcc", "SFXWorldResources.pcc", "SFXVehicleResources.pcc", "Startup_DE.pcc",
        "Startup_ES.pcc", "Startup_FE.pcc", "Startup_FR.pcc", "Startup_GE.pcc",
        "Startup_IE.pcc", "Startup_INT.pcc", "Startup_IT.pcc", "Startup_JA.pcc",
        "Startup_PL.pcc", "Startup_PLPC.pcc", "Startup_RA.pcc", "Startup_RU.pcc",
        "EntryMenu.pcc", "EntryMenu_LOC_DE.pcc", "EntryMenu_LOC_FR.pcc", "EntryMenu_LOC_INT.pcc",
        "EntryMenu_LOC_IT.pcc", "EntryMenu_LOC_PLPC.pcc", "EntryMenu_LOC_RA.pcc",
    };

    private static readonly string[] Le2Targets =
    {
        "Core.pcc", "Engine.pcc", "IpDrv.pcc", "GFxUI.pcc",
        "WwiseAudio.pcc", "SFXOnlineFoundation.pcc", "PlotManagerMap.pcc", "PlotManagerMap_LOC_INT.pcc",
        "SFXGame.pcc", "Startup_DEU.pcc", "Startup_ESN.pcc", "Startup_FRA.pcc",
        "Startup_INT.pcc", "Startup_ITA.pcc", "Startup_JPN.pcc", "Startup_POL.pcc",
        "Startup_RUS.pcc", "EntryMenu.pcc", "EntryMenu_LOC_DEU.pcc", "EntryMenu_LOC_FRA.pcc",
        "EntryMenu_LOC_INT.pcc", "EntryMenu_LOC_ITA.pcc", "EntryMenu_LOC_POL.pcc",
    };

    private static readonly string[] Le3Targets =
    {
        "Core.pcc", "Engine.pcc", "GameFramework.pcc", "IpDrv.pcc",
        "GFxUI.pcc", "WwiseAudio.pcc", "SFXOnlineFoundation.pcc", "SFXGame.pcc",
        "Startup.pcc", "EntryMenu.pcc",
    };

    internal static string[] Targets(MEGame game) => game switch
    {
        MEGame.LE1 => Le1Targets,
        MEGame.LE2 => Le2Targets,
        MEGame.LE3 => Le3Targets,
        _ => throw new InvalidDataException("M3M requires a Legendary Edition game."),
    };

    internal static AssetMergeRequest ReadRequest(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (stream.Length > 4 * 1024 * 1024)
            throw new InvalidDataException("Asset merge request exceeds its size limit.");
        using var document = JsonDocument.Parse(stream, new JsonDocumentOptions { MaxDepth = 16 });
        TransformProtocol.RejectDuplicateKeys(document.RootElement);
        var request = document.Deserialize<AssetMergeRequest>(TransformProtocol.Json)
            ?? throw new InvalidDataException("An asset merge request is required.");
        Validate(request);
        return request;
    }

    internal static void Validate(AssetMergeRequest request)
    {
        if (request.Protocol != 1 || request.Operation != "le1-m3m-assets"
            || request.Targets is null || request.Targets.Length < 1 || request.Targets.Length > Le1Targets.Length
            || request.Assets is null || request.Assets.Length is < 1 or > 1024
            || request.Merges is null || request.Merges.Length is < 1 or > 4096)
            throw new InvalidDataException("Invalid or unsupported asset merge request.");
        foreach (string root in new[] { request.GameRoot, request.InputRoot, request.OutputRoot })
            TransformProtocol.ValidateRoot(root);
        if (TransformProtocol.Overlaps(request.GameRoot, request.OutputRoot)
            || TransformProtocol.Overlaps(request.InputRoot, request.OutputRoot)
            || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any())
            throw new InvalidDataException("Asset merge output must be empty and separate from all inputs.");
        var targets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var assets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long total = 0;
        foreach (var input in request.Targets.Concat(request.Assets))
        {
            if (input is null || input.Size is < 1 or > 512 * 1024 * 1024)
                throw new InvalidDataException("Invalid asset merge input size.");
            total = checked(total + input.Size);
            if (total > 2L * 1024 * 1024 * 1024)
                throw new InvalidDataException("Asset merge inputs exceed the job size limit.");
            TransformProtocol.Relative(input.Path);
        }
        foreach (var input in request.Targets)
            if (!input.Path.StartsWith("CookedPCConsole/", StringComparison.Ordinal)
                || input.Path.Split('/').Length != 2
                || !Le1Targets.Contains(Path.GetFileName(input.Path), StringComparer.OrdinalIgnoreCase)
                || !targets.Add(input.Path))
                throw new InvalidDataException("Invalid or duplicate asset merge target.");
        foreach (var input in request.Assets)
            if (!input.Path.StartsWith("Assets/", StringComparison.Ordinal)
                || !input.Path.EndsWith(".pcc", StringComparison.OrdinalIgnoreCase)
                || !assets.Add(input.Path))
                throw new InvalidDataException("Invalid or duplicate embedded package input.");
        var usedTargets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var usedAssets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var merge in request.Merges)
        {
            if (merge is null || !targets.Contains(merge.Target) || !assets.Contains(merge.Asset))
                throw new InvalidDataException("Asset merge references an undeclared input.");
            EntryName(merge.Entry);
            EntryName(merge.SourceEntry);
            usedTargets.Add(merge.Target);
            usedAssets.Add(merge.Asset);
        }
        if (!targets.SetEquals(usedTargets) || !assets.SetEquals(usedAssets))
            throw new InvalidDataException("Asset merge request contains unused inputs.");
    }

    internal static void EntryName(string name)
    {
        if (string.IsNullOrEmpty(name) || name.Length > 1024 || name.Split('.').Any(part => part.Length == 0
            || part.Any(character => !char.IsAsciiLetterOrDigit(character) && character != '_')))
            throw new InvalidDataException("Invalid asset merge export path.");
    }

    internal static void Apply(IMEPackage source, IMEPackage target, AssetMerge merge, string resolutionRoot)
    {
        if (source.Game is not (MEGame.LE1 or MEGame.LE2 or MEGame.LE3) || target.Game != source.Game)
            throw new InvalidDataException("Asset merging requires matching Legendary Edition games.");
        var from = source.FindExport(merge.SourceEntry)
            ?? throw new InvalidDataException("The embedded package does not contain the requested export.");
        var destination = target.FindExport(merge.Entry);
        using var cache = new DeclaredPackageCache();
        RelinkerOptionsPackage Options(bool parents = false) => new()
        {
            Cache = cache,
            GamePathOverride = resolutionRoot,
            GenerateImportsForGlobalFiles = false,
            ImportChildrenOfPackages = !parents,
            ErrorOccurredCallback = _ => throw new InvalidDataException("Asset references could not be relinked."),
        };
        if (destination is not null)
        {
            if (destination.ClassName != from.ClassName)
                throw new InvalidDataException("Asset replacement cannot change the target export class.");
            var errors = EntryImporter.ImportAndRelinkEntries(EntryImporter.PortingOption.ReplaceSingularWithRelink,
                from, target, destination, true, Options(), out _);
            if (errors.Count != 0)
                throw new InvalidDataException("Asset replacement could not relink every reference.");
        }
        else
        {
            if (!merge.AllowNew || !string.Equals(merge.Entry, merge.SourceEntry, StringComparison.OrdinalIgnoreCase))
                throw new InvalidDataException("The requested target export is missing or cannot be added at that path.");
            var ancestors = new Stack<IEntry>();
            for (IEntry entry = from.Parent; entry is not null; entry = entry.Parent)
                ancestors.Push(entry);
            IEntry parent = null;
            foreach (var ancestor in ancestors)
            {
                var existing = target.FindEntry(ancestor.InstancedFullPath);
                if (existing is not null)
                {
                    if (existing.ClassName != ancestor.ClassName)
                        throw new InvalidDataException("An asset parent has an incompatible class.");
                    parent = existing;
                }
                else
                {
                    var errors = EntryImporter.ImportAndRelinkEntries(EntryImporter.PortingOption.CloneAllDependencies,
                        ancestor, target, parent, true, Options(parents: true), out parent);
                    if (errors.Count != 0 || parent is null)
                        throw new InvalidDataException("Asset parent references could not be relinked.");
                }
            }
            var result = EntryImporter.ImportAndRelinkEntries(EntryImporter.PortingOption.CloneAllDependencies,
                from, target, parent, true, Options(), out var added);
            if (result.Count != 0 || added is not ExportEntry
                || !string.Equals(added.InstancedFullPath, merge.Entry, StringComparison.OrdinalIgnoreCase))
                throw new InvalidDataException("The new asset could not be added at the requested export path.");
        }
    }

    internal static OutputFile[] Execute(AssetMergeRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        cancellation.ThrowIfCancellationRequested();
        if (!OodleHelper.EnsureOodleDll(request.GameRoot))
            throw new InvalidDataException("The verified game codec is unavailable.");
        var packages = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        string resolutionRoot = Path.Combine(request.OutputRoot, ".resolution");
        Directory.CreateDirectory(resolutionRoot);
        try
        {
            foreach (var identity in request.Targets.Concat(request.Assets))
            {
                cancellation.ThrowIfCancellationRequested();
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, identity);
                var package = MEPackageHandler.OpenMEPackageFromStream(bytes, Path.GetFileName(identity.Path));
                packages.Add(identity.Path, package);
                if (package.Game != MEGame.LE1)
                    throw new InvalidDataException("Asset merge input is not an LE1 package.");
            }
            for (int index = 0; index < request.Merges.Length; index++)
            {
                cancellation.ThrowIfCancellationRequested();
                var merge = request.Merges[index];
                Apply(packages[merge.Asset], packages[merge.Target], merge, resolutionRoot);
                progress(index + 1, request.Merges.Length);
            }
            return request.Targets.Select(target => PackageOutput.Write(packages[target.Path],
                request.OutputRoot, target.Path, cancellation)).ToArray();
        }
        finally
        {
            foreach (var package in packages.Values)
                package.Dispose();
            Directory.Delete(resolutionRoot, recursive: true);
        }
    }

    private sealed class DeclaredPackageCache : PackageCache
    {
        public override IMEPackage GetCachedPackage(string packagePath, bool openIfNotInCache = true,
            Func<string, IMEPackage> openPackageMethod = null) =>
            throw new InvalidDataException("Asset merging requires an undeclared external package.");
    }
}
