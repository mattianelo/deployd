using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore;
using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.GameFilesystem;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.ObjectInfo;
using LegendaryExplorerCore.UnrealScript;

namespace Deployd.Mele;

internal static class OrderedM3mTests
{
    // @variants: both
    internal static void Run(string root)
    {
        string input = Path.Combine(root, "ordered-input");
        string original = Path.Combine(root, "ordered-original");
        string output = Path.Combine(root, "ordered-output");
        foreach (string path in new[] { input, original, output }) Directory.CreateDirectory(path);
        var file = new InputFile("CookedPCConsole/Core.pcc", 4, new string('0', 64));
        var script = file with { Path = "Scripts/class.uc" };
        var job = new M3mJob(file.Path, "NewClass", "class", script.Path, "", false);
        var request = new M3mRequest(1, "mele-m3m-ordered", "LE1", input, original, input, output,
            new[] { new TargetPackage(file, file) }, Array.Empty<InputFile>(), Array.Empty<InputFile>(), new[] { script }, new[] { job });
        M3mOrdered.Validate(request);
        foreach (string game in new[] { "LE2", "LE3" })
            M3mOrdered.Validate(request with { Game = game });
        foreach (string game in new[] { "ME1", "", null })
            Reject(() => M3mOrdered.Validate(request with { Game = game }));
        Reject(() => M3mOrdered.Validate(request with { Protocol = 2 }));
        Reject(() => M3mOrdered.Validate(request with { OriginalRoot = input }));
        Reject(() => M3mOrdered.Validate(request with { OutputRoot = original }));
        string linked = Path.Combine(root, "ordered-linked");
        Directory.CreateSymbolicLink(linked, original);
        Reject(() => M3mOrdered.Validate(request with { OriginalRoot = linked }));
        File.WriteAllText(Path.Combine(output, "existing"), "preserve");
        Reject(() => M3mOrdered.Validate(request));
        File.Delete(Path.Combine(output, "existing"));
        foreach (string anchor in new[] { "../Core.pcc", "../system/Core.pcc", "~docs~/Core.pcc", "Mods/Core.pcc" })
        {
            var outside = file with { Path = anchor };
            Reject(() => M3mOrdered.Validate(request with { Targets = new[] { new TargetPackage(outside, outside) },
                Jobs = new[] { job with { Target = anchor } } }));
        }
        Reject(() => M3mOrdered.Validate(request with { Targets = new[] { request.Targets[0], request.Targets[0] } }));
        Reject(() => M3mOrdered.Validate(request with { Dependencies = new[] { file } }));
        Reject(() => M3mOrdered.Validate(request with { Scripts = new[] { script, script } }));
        Reject(() => M3mOrdered.Validate(request with { Scripts = new[] { script with { Size = 4 * 1024 * 1024 + 1 } } }));
        Reject(() => M3mOrdered.Validate(request with { Assets = new[] { file with { Path = "Assets/../escape.pcc" } } }));
        Reject(() => M3mOrdered.Validate(request with { Assets = new[] { file with { Path = "Assets/unused.pcc" } } }));
        Reject(() => M3mOrdered.Validate(request with { Jobs = new[] { job with { Kind = "future" } } }));
        Reject(() => M3mOrdered.Validate(request with { Jobs = new[] { job with { Entry = "A.B.C" } } }));
        Reject(() => M3mOrdered.Validate(request with { Jobs = new[] { job with { Entry = "../Escape" } } }));
        Reject(() => M3mOrdered.Validate(request with { Jobs = new[] { job with { Input = "Scripts/missing.uc" } } }));
        Reject(() => M3mOrdered.Validate(request with { Jobs = new[] { job with { AllowNew = true } } }));
        Reject(() => M3mOrdered.Validate(request with { Jobs = new[] { job with { SourceEntry = "Ignored" } } }));
        Reject(() => M3mOrdered.Validate(request with { Jobs = new[] { job with { Kind = "asset" } } }));
        var sfx = file with { Path = "CookedPCConsole/SFXGame.pcc" };
        Reject(() => M3mOrdered.Validate(request with { Targets = new[] { new TargetPackage(sfx, sfx) },
            Jobs = new[] { job with { Target = sfx.Path } } }));
        var asset = file with { Path = "Assets/Example.pcc" };
        foreach (string game in new[] { "LE2", "LE3" })
            M3mOrdered.Validate(request with { Game = game, Assets = new[] { asset }, Scripts = Array.Empty<InputFile>(),
                Jobs = new[] { new M3mJob(file.Path, "Example", "asset", asset.Path, "Example", false) } });
        foreach (var game in new[] { MEGame.LE1, MEGame.LE2, MEGame.LE3 })
        {
            foreach (string name in EntryImporter.FilesSafeToImportFrom(game).Append("EntryMenu.pcc"))
                Require(M3mAssets.Targets(game).Contains(name, StringComparer.OrdinalIgnoreCase),
                    $"Missing upstream M3M target for {game}: {name}");
            foreach (string name in M3mAssets.Targets(game))
            {
                var target = file with { Path = "CookedPCConsole/" + name };
                M3mOrdered.Validate(request with { Game = game.ToString(),
                    Targets = new[] { new TargetPackage(target, target) }, Assets = new[] { asset },
                    Scripts = Array.Empty<InputFile>(),
                    Jobs = new[] { new M3mJob(target.Path, "Example", "asset", asset.Path, "Example", false) } });
            }
            foreach (string name in new[] { "Startup.pcc", "Startup_FRA.pcc", "EntryMenu_LOC_PLPC.pcc",
                "EngineTest.pcc", "Startup_MOD_TEST_INT.pcc", "Startup.pcc.extra" })
            {
                var target = file with { Path = "CookedPCConsole/" + name };
                var candidate = request with { Game = game.ToString(),
                    Targets = new[] { new TargetPackage(target, target) }, Assets = new[] { asset },
                    Scripts = Array.Empty<InputFile>(),
                    Jobs = new[] { new M3mJob(target.Path, "Example", "asset", asset.Path, "Example", false) } };
                bool valid = (game, name) is (MEGame.LE3, "Startup.pcc")
                    or (MEGame.LE2, "Startup_FRA.pcc") or (MEGame.LE1, "EntryMenu_LOC_PLPC.pcc");
                if (valid) M3mOrdered.Validate(candidate);
                else Reject(() => M3mOrdered.Validate(candidate));
            }
        }
        foreach (string code in new[] { "DE", "ES", "FE", "FR", "GE", "IE", "INT", "IT", "JA", "PL", "PLPC", "RA", "RU" })
        {
            var localized = file with { Path = $"CookedPCConsole/Startup_{code}.pcc" };
            var localizedJob = new M3mJob(localized.Path, "Example", "asset", asset.Path, "Example", false);
            M3mOrdered.Validate(request with { Targets = new[] { new TargetPackage(localized, localized) },
                Assets = new[] { asset }, Scripts = Array.Empty<InputFile>(), Jobs = new[] { localizedJob } });
            M3mAssets.Validate(new AssetMergeRequest(1, "le1-m3m-assets", input, input, output,
                new[] { localized }, new[] { asset }, new[] { new AssetMerge(localized.Path, "Example", asset.Path, "Example", false) }));
            Reject(() => M3mOrdered.Validate(request with { Targets = new[] { new TargetPackage(localized, localized) },
                Jobs = new[] { job with { Target = localized.Path } } }));
        }
        var combinedTargets = M3mAssets.Targets(MEGame.LE1).Select(name => file with { Path = "CookedPCConsole/" + name }).ToArray();
        M3mOrdered.Validate(request with { Targets = combinedTargets.Select(target => new TargetPackage(target, target)).ToArray(),
            Assets = new[] { asset }, Scripts = Array.Empty<InputFile>(),
            Jobs = combinedTargets.Select(target => new M3mJob(target.Path, "Example", "asset", asset.Path, "Example", false)).ToArray() });
        M3mAssets.Validate(new AssetMergeRequest(1, "le1-m3m-assets", input, input, output, combinedTargets, new[] { asset },
            combinedTargets.Select(target => new AssetMerge(target.Path, "Example", asset.Path, "Example", false)).ToArray()));
        foreach (string name in new[] { "Startup_FRA.pcc", "Startup_XX.pcc", "Startup.pcc", "Startup_INT.pcc.extra" })
        {
            var unknown = file with { Path = "CookedPCConsole/" + name };
            Reject(() => M3mOrdered.Validate(request with { Targets = new[] { new TargetPackage(unknown, unknown) },
                Assets = new[] { asset }, Scripts = Array.Empty<InputFile>(),
                Jobs = new[] { new M3mJob(unknown.Path, "Example", "asset", asset.Path, "Example", false) } }));
            Reject(() => M3mAssets.Validate(new AssetMergeRequest(1, "le1-m3m-assets", input, input, output,
                new[] { unknown }, new[] { asset }, new[] { new AssetMerge(unknown.Path, "Example", asset.Path, "Example", false) })));
        }
        string json = Path.Combine(input, "request.json");
        string text = JsonSerializer.Serialize(request, TransformProtocol.Json);
        foreach (string invalid in new[] { text.Replace("\"protocol\":1", "\"protocol\":1,\"protocol\":1"),
            text.Replace("\"protocol\":1", "\"protocol\":1,\"future\":true") })
        {
            File.WriteAllText(json, invalid);
            Reject(() => M3mOrdered.ReadRequest(json));
        }
        using var cancelled = new CancellationTokenSource();
        cancelled.Cancel();
        try
        {
            M3mOrdered.Execute(request, cancelled.Token, (_, _) => throw new InvalidDataException("Cancelled job progressed."));
            throw new InvalidDataException("Cancelled ordered M3M request ran.");
        }
        catch (OperationCanceledException) { }
        Require(!Directory.EnumerateFileSystemEntries(output).Any(), "Cancelled job published output.");
        using var package = MEPackageHandler.CreateMemoryEmptyPackage("Core.pcc", MEGame.LE1);
        var originals = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "Object" };
        Reject(() => M3mClasses.Apply(package, "OBJECT", "class Object;", originals, null, null));
        Reject(() => M3mClasses.Apply(package, "One.Two.Three", "class Three;", originals, null, null));
        using var wrongGame = MEPackageHandler.CreateMemoryEmptyPackage("Core.pcc", MEGame.ME2);
        Reject(() => M3mClasses.Apply(wrongGame, "Custom", "class Custom;", originals, null, null));
        Console.WriteLine("Ordered M3M request, original-class protection, and cancellation tests passed.");
    }

    internal static void VerifyCorpus(string root)
    {
        var request = M3mOrdered.ReadRequest(Path.Combine(root, "semantic-request.json"));
        Require(OodleHelper.EnsureOodleDll(request.GameRoot), "Verified codec unavailable.");
        if (request.Game == "LE1") LE1UnrealObjectInfo.ObjectInfo.LoadData(null);
        else if (request.Game == "LE2") LE2UnrealObjectInfo.ObjectInfo.LoadData(null);
        else LE3UnrealObjectInfo.ObjectInfo.LoadData(null);
        foreach (var target in request.Targets)
        {
            string path = target.Current.Path;
            using var original = MEPackageHandler.OpenMEPackage(Path.Combine(request.OriginalRoot, path), forceLoadFromDisk: true);
            using var merged = MEPackageHandler.OpenMEPackage(Path.Combine(root, "merged", path), forceLoadFromDisk: true);
            using var reapplied = MEPackageHandler.OpenMEPackage(Path.Combine(root, "reapplied", path), forceLoadFromDisk: true);
            Require(merged.ExportCount == reapplied.ExportCount && merged.Names.SequenceEqual(reapplied.Names), "Reapplication changed package tables.");
            var jobs = request.Jobs.Where(job => job.Target == path).ToArray();
            var changed = jobs.Select(job => job.Kind == "function" ? job.Entry[..job.Entry.LastIndexOf('.')] : job.Entry).ToArray();
            foreach (var entry in original.Exports)
            {
                var after = entry.UIndex <= merged.ExportCount ? merged.GetUExport(entry.UIndex) : null;
                Require(after is not null && after.ClassName == entry.ClassName && after.InstancedFullPath == entry.InstancedFullPath, $"Original export identity changed at index {entry.UIndex}: {entry.InstancedFullPath}.");
                if (!changed.Any(name => entry.InstancedFullPath == name
                    || entry.InstancedFullPath.StartsWith(name + ".", StringComparison.OrdinalIgnoreCase)
                    || entry.InstancedFullPath.StartsWith("Default__" + name, StringComparison.OrdinalIgnoreCase)))
                {
                    if (entry.ClassName == "ShaderCache") CommunityPatchTests.SameShaderCache(entry, after);
                    else Require(entry.Data.AsSpan().SequenceEqual(after.Data), $"Unrelated original export changed: {entry.InstancedFullPath}.");
                }
            }
            foreach (var entry in merged.Exports)
            {
                var again = entry.UIndex <= reapplied.ExportCount ? reapplied.GetUExport(entry.UIndex) : null;
                Require(again is not null && again.ClassName == entry.ClassName && again.InstancedFullPath == entry.InstancedFullPath, "Reapplication changed an export identity.");
                if (entry.ClassName == "ShaderCache") CommunityPatchTests.SameShaderCache(entry, again);
                else if (entry.ClassName == "Texture2D" && jobs.Any(job => job.Kind == "asset" && job.Entry == entry.InstancedFullPath))
                    AssetMergeTests.SameTexture(entry, again, original.FindExport(entry.InstancedFullPath).DataOffset, entry.DataOffset);
                else Require(entry.Data.AsSpan().SequenceEqual(again.Data), $"Reapplication changed an export: {entry.InstancedFullPath}.");
            }
        }
        Reference(root, request);
        using var cancellation = new CancellationTokenSource();
        int cancelAfter = Math.Max(1, Array.FindIndex(request.Jobs, job => job.Kind == "class") + 1);
        try
        {
            M3mOrdered.Execute(request, cancellation.Token, (completed, _) => { if (completed == cancelAfter) cancellation.Cancel(); });
            throw new InvalidDataException("Cancellation after an applied operation was ignored.");
        }
        catch (OperationCanceledException) { }
        Require(!Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "Cancelled combined job published output.");
        var altered = request.Targets.Select((target, index) => index == 0
            ? target with { Original = target.Original with { Sha256 = new string('0', 64) } } : target).ToArray();
        bool advanced = false;
        Reject(() => M3mOrdered.Execute(request with { Targets = altered }, CancellationToken.None, (_, _) => advanced = true));
        Require(!advanced && !Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(),
            "A changed original reached transformation or published output.");
        Console.WriteLine($"Verified {request.Jobs.Length} ordered operations, pinned reference, reapplication, and cancellation after operation {cancelAfter}.");
    }

    internal static void VerifyStartupCorpus(string root)
    {
        foreach (string name in M3mAssets.Targets(MEGame.LE1).Where(name => name.StartsWith("Startup_", StringComparison.Ordinal)))
        {
            VerifyCorpus(Path.Combine(root, Path.GetFileNameWithoutExtension(name)));
            Console.WriteLine($"Localized Startup corpus passed: {name}");
        }
    }

    private static void Reference(string root, M3mRequest request)
    {
        string resolution = Path.Combine(request.OutputRoot, "reference");
        Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "CookedPCConsole"));
        Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "DLC"));
        var packages = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        var assets = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        string previousRoot = LE1Directory.DefaultGamePath;
        string previousLe2 = LE2Directory.DefaultGamePath;
        string previousLe3 = LE3Directory.DefaultGamePath;
        string previousFamily = LegendaryExplorerCoreLibSettings.Instance.LEDirectory;
        LegendaryExplorerCoreLibSettings.Instance.LEDirectory = null;
        LE1Directory.DefaultGamePath = null;
        LE2Directory.DefaultGamePath = null;
        LE3Directory.DefaultGamePath = null;
        try
        {
            foreach (var input in request.Dependencies.Concat(request.Targets.Select(target => target.Current)))
            {
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
                packages.Add(Path.GetFileName(input.Path), MEPackageHandler.OpenMEPackageFromStream(bytes,
                    Path.Combine(resolution, "BioGame", "CookedPCConsole", Path.GetFileName(input.Path))));
            }
            foreach (var input in request.Assets)
            {
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
                assets.Add(input.Path, MEPackageHandler.OpenMEPackageFromStream(bytes, Path.GetFileName(input.Path)));
            }
            using var cache = new ScriptPackageCache(packages, resolution);
            var options = new UnrealScriptOptionsPackage { Cache = cache, GamePathOverride = resolution,
                CustomFileResolver = (name, _) => cache.ResolveCandidate(name) };
            foreach (var job in request.Jobs)
            {
                var target = packages[Path.GetFileName(job.Target)];
                if (job.Kind == "asset")
                {
                    var existing = target.FindExport(job.Entry);
                    var errors = EntryImporter.ImportAndRelinkEntries(existing is null
                        ? EntryImporter.PortingOption.CloneAllDependencies : EntryImporter.PortingOption.ReplaceSingularWithRelink,
                        assets[job.Input].FindExport(job.SourceEntry), target, existing, true,
                        new RelinkerOptionsPackage { Cache = cache, GamePathOverride = resolution, GenerateImportsForGlobalFiles = false }, out _);
                    Require(errors.Count == 0, "Pinned asset reference failed.");
                }
                else
                {
                    using var library = new FileLib(target);
                    Require(library.Initialize(options, canUseBinaryCache: false), "Pinned compiler could not initialize.");
                    string script = File.ReadAllText(Path.Combine(request.InputRoot, job.Input));
                    var log = job.Kind switch
                    {
                        "class" => UnrealScriptCompiler.CompileClass(target, script, library, options,
                            export: target.FindExport(job.Entry), intendedClassName: job.Entry).log,
                        "function" => UnrealScriptCompiler.CompileFunction(target.FindExport(job.Entry), script, library, options).log,
                        _ => UnrealScriptCompiler.AddOrReplaceInClass(target.FindExport(job.Entry), script, library, options),
                    };
                    Require(!log.HasErrors && !log.HasLexErrors, $"Pinned compiler reference failed: {log}");
                    Require(library.ReInitializeFile(options), "Pinned symbols could not be rebuilt.");
                }
            }
            foreach (var target in request.Targets)
            {
                var expected = packages[Path.GetFileName(target.Current.Path)];
                using var serialized = expected.SaveToStream(compress: true);
                using var actual = MEPackageHandler.OpenMEPackage(Path.Combine(root, "merged", target.Current.Path), forceLoadFromDisk: true);
                ScriptMergeTests.SamePackage(expected, actual);
            }
            if (request.Jobs.Any(job => job.Kind == "class"))
            {
                var sfx = packages["SFXGame.pcc"];
                using var classLibrary = new FileLib(sfx);
                Require(classLibrary.Initialize(options, canUseBinaryCache: false), "Class test symbols unavailable.");
                var originals = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "Object", "BioPawn" };
                M3mClasses.Apply(sfx, "DeploydClassContainer.DeploydClassTest", "class DeploydClassTest extends Object;", originals, classLibrary, options);
                Require(sfx.FindExport("DeploydClassContainer").IsForcedExport, "A new class container is not a forced export.");
                Require(sfx.FindExport("DeploydClassContainer.DeploydClassTest").IsClass, "Nested class was not created.");
                int count = sfx.ExportCount;
                M3mClasses.Apply(sfx, "DeploydClassContainer.DeploydClassTest", "class DeploydClassTest extends Object;", originals, classLibrary, options);
                Require(sfx.ExportCount == count, "Reapplying a nested class duplicated exports.");
                Reject(() => M3mClasses.Apply(sfx, "BioPawn", "class BioPawn extends Object;", originals, classLibrary, options));
                Reject(() => M3mClasses.Apply(sfx, "DifferentClass", "class AnotherClass extends Object;", originals, classLibrary, options));
            }
        }
        finally
        {
            LegendaryExplorerCoreLibSettings.Instance.LEDirectory = previousFamily;
            LE1Directory.DefaultGamePath = previousRoot;
            LE2Directory.DefaultGamePath = previousLe2;
            LE3Directory.DefaultGamePath = previousLe3;
            foreach (var package in packages.Values.Concat(assets.Values)) package.Dispose();
            Directory.Delete(resolution, recursive: true);
        }
    }

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidDataException(message);
    }

    private static void Reject(Action action)
    {
        try { action(); }
        catch (InvalidDataException) { return; }
        catch (JsonException) { return; }
        throw new InvalidDataException("Invalid ordered M3M operation was accepted.");
    }
}
