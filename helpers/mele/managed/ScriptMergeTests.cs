using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore;
using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.GameFilesystem;
using LegendaryExplorerCore.Unreal.ObjectInfo;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.UnrealScript;

namespace Deployd.Mele;

internal static class ScriptMergeTests
{
    // @variants: both
    internal static void Run(string root)
    {
        string input = Path.Combine(root, "script-input");
        string output = Path.Combine(root, "script-output");
        Directory.CreateDirectory(input);
        Directory.CreateDirectory(output);
        var file = new InputFile("CookedPCConsole/Core.pcc", 4, new string('0', 64));
        var script = file with { Path = "Scripts/change.uc" };
        var merge = new ScriptMerge("Object.GetValue", "function", script.Path);
        var request = new ScriptMergeRequest(1, "le1-m3m-scripts", input, input, output,
            file, Array.Empty<InputFile>(), new[] { script }, new[] { merge });
        M3mScripts.Validate(request);
        Reject(() => M3mScripts.Validate(request with { Protocol = 2 }));
        Reject(() => M3mScripts.Validate(request with { Operation = "le2-m3m-scripts" }));
        Reject(() => M3mScripts.Validate(request with { OutputRoot = input }));
        string linked = Path.Combine(root, "script-linked");
        Directory.CreateSymbolicLink(linked, input);
        Reject(() => M3mScripts.Validate(request with { InputRoot = linked }));
        File.WriteAllText(Path.Combine(output, "existing"), "preserve");
        Reject(() => M3mScripts.Validate(request));
        File.Delete(Path.Combine(output, "existing"));
        Reject(() => M3mScripts.Validate(request with { Target = file with { Path = "CookedPCConsole/Startup_INT.pcc" } }));
        Reject(() => M3mScripts.Validate(request with { Target = file with { Path = "CookedPCConsole/SFXGame.pcc" } }));
        Reject(() => M3mScripts.Validate(request with { Dependencies = new[] { file } }));
        Reject(() => M3mScripts.Validate(request with { Scripts = new[] { script, script } }));
        Reject(() => M3mScripts.Validate(request with { Scripts = new[] { script with { Path = "Scripts/../change.uc" } } }));
        Reject(() => M3mScripts.Validate(request with { Scripts = new[] { script with { Size = 4 * 1024 * 1024 + 1 } } }));
        Reject(() => M3mScripts.Validate(request with { Merges = new[] { merge with { Kind = "class" } } }));
        Reject(() => M3mScripts.Validate(request with { Merges = new[] { merge with { Entry = "Object..Function" } } }));
        Reject(() => M3mScripts.Validate(request with { Merges = new[] { merge with { Script = "Scripts/missing.uc" } } }));
        Reject(() => M3mScripts.Validate(request with { Scripts = new[] { script, script with { Path = "Scripts/unused.uc" } } }));
        string json = Path.Combine(input, "request.json");
        string text = JsonSerializer.Serialize(request, TransformProtocol.Json);
        foreach (string invalid in new[] { text.Replace("\"protocol\":1", "\"protocol\":1,\"protocol\":1"),
            text.Replace("\"protocol\":1", "\"protocol\":1,\"future\":true"), text.Replace("\"protocol\":1,", "") })
        {
            File.WriteAllText(json, invalid);
            Reject(() => M3mScripts.ReadRequest(json));
        }
        using var cancellation = new CancellationTokenSource();
        cancellation.Cancel();
        try
        {
            M3mScripts.Execute(request, cancellation.Token, (_, _) => throw new InvalidDataException("Unexpected progress."));
            throw new InvalidDataException("Cancelled script job ran.");
        }
        catch (OperationCanceledException) { }
        Require(!Directory.EnumerateFileSystemEntries(output).Any(), "Cancelled job wrote outputs.");
        byte[] invalidText = { 0xff };
        Directory.CreateDirectory(Path.Combine(input, "Scripts"));
        File.WriteAllBytes(Path.Combine(input, script.Path), invalidText);
        var invalidScript = script with { Size = invalidText.Length, Sha256 = Convert.ToHexStringLower(SHA256.HashData(invalidText)) };
        try
        {
            M3mScripts.Execute(request with { Scripts = new[] { invalidScript } }, CancellationToken.None, (_, _) => { });
            throw new InvalidDataException("Invalid UTF-8 source was accepted.");
        }
        catch (InvalidDataException error)
        {
            Require(error.Message == "Script inputs must contain valid UTF-8 source text.", "Invalid UTF-8 did not fail before codec loading.");
        }
        using var package = MEPackageHandler.CreateMemoryEmptyPackage("Core.pcc", MEGame.LE1);
        using var cache = new ScriptPackageCache(new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase) { ["Core.pcc"] = package }, output);
        Require(ReferenceEquals(cache.GetCachedPackage("CORE.pcc"), package), "Declared dependency was not resolved.");
        Require(cache.ResolveCandidate("Missing.pcc") is null, "An absent import candidate was fabricated.");
        Require(ReferenceEquals(cache.GetCachedPackage("CookedPCConsole\\Core.pcc"), package),
            "Pinned Windows struct metadata could not resolve a declared package.");
        Require(ReferenceEquals(cache.GetCachedPackage(Path.Combine(output, "BioGame", "CookedPCConsole", "Core.pcc")), package),
            "Private compiler path did not resolve a declared package.");
        Reject(() => cache.ResolveCandidate("../Core.pcc"));
        Reject(() => cache.ResolveCandidate("CookedPCConsole\\Core.pcc"));
        Reject(() => cache.GetCachedPackage("Missing.pcc"));
        Reject(() => cache.GetCachedPackage("../Core.pcc"));
        Reject(() => cache.GetCachedPackage(Path.Combine(input, "Core.pcc")));
        Console.WriteLine("Script request, dependency isolation, and cancellation tests passed.");
    }

    internal static void VerifyCorpus(string root)
    {
        LE1UnrealObjectInfo.ObjectInfo.LoadData(null);
        var request = M3mScripts.ReadRequest(Path.Combine(root, "semantic-request.json"));
        Require(OodleHelper.EnsureOodleDll(request.GameRoot), "Verified codec unavailable.");
        string relative = request.Target.Path;
        using var original = MEPackageHandler.OpenMEPackage(Path.Combine(root, "input", relative), forceLoadFromDisk: true);
        using var merged = MEPackageHandler.OpenMEPackage(Path.Combine(root, "merged", relative), forceLoadFromDisk: true);
        using var reapplied = MEPackageHandler.OpenMEPackage(Path.Combine(root, "reapplied", relative), forceLoadFromDisk: true);
        SamePackage(merged, reapplied);
        var changedClasses = request.Merges.Select(merge => merge.Kind == "member" ? merge.Entry
            : merge.Entry[..merge.Entry.LastIndexOf('.')]).ToHashSet(StringComparer.OrdinalIgnoreCase);
        int changed = 0;
        foreach (var before in original.Exports)
        {
            var after = merged.FindExport(before.InstancedFullPath);
            Require(after is not null && before.ClassName == after.ClassName, "An original export was lost or changed class.");
            if (before.ClassName == "ShaderCache")
                CommunityPatchTests.SameShaderCache(before, after);
            else if (!before.Data.AsSpan().SequenceEqual(after.Data))
            {
                Require(changedClasses.Any(name => before.InstancedFullPath == name
                    || before.InstancedFullPath.StartsWith(name + ".", StringComparison.OrdinalIgnoreCase)
                    || before.InstancedFullPath.StartsWith("Default__" + name, StringComparison.OrdinalIgnoreCase)),
                    $"Unrelated export changed: {before.InstancedFullPath}.");
                changed++;
            }
        }
        Require(changed >= request.Merges.Length, "Expected script changes were not compiled.");
        var packages = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        string resolution = Path.Combine(request.OutputRoot, "reference");
        string previousRoot = LE1Directory.DefaultGamePath;
        string previousFamilyRoot = LegendaryExplorerCoreLibSettings.Instance.LEDirectory;
        try
        {
            foreach (var input in request.Dependencies.Concat(new[] { request.Target }))
            {
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
                string name = Path.GetFileName(input.Path);
                packages.Add(name, MEPackageHandler.OpenMEPackageFromStream(bytes, Path.Combine(resolution, "BioGame", "CookedPCConsole", name)));
            }
            Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "CookedPCConsole"));
            Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "DLC"));
            LegendaryExplorerCoreLibSettings.Instance.LEDirectory = null;
            LE1Directory.DefaultGamePath = null;
            using var cache = new ScriptPackageCache(packages, resolution);
            var options = new UnrealScriptOptionsPackage { GamePathOverride = resolution, Cache = cache,
                CustomFileResolver = (name, _) => cache.ResolveCandidate(name) };
            var target = packages[Path.GetFileName(relative)];
            using var library = new FileLib(target);
            Require(library.Initialize(options, canUseBinaryCache: false), "Reference compiler initialization failed.");
            foreach (var merge in request.Merges)
            {
                string script = File.ReadAllText(Path.Combine(request.InputRoot, merge.Script));
                var entry = target.FindExport(merge.Entry);
                var log = merge.Kind == "function"
                    ? UnrealScriptCompiler.CompileFunction(entry, script, library, options).log
                    : UnrealScriptCompiler.AddOrReplaceInClass(entry, script, library, options);
                Require(!log.HasErrors && !log.HasLexErrors, $"Pinned reference compilation failed: {log}");
                Require(library.ReInitializeFile(options), "Reference symbols could not be rebuilt.");
            }
            using var serialized = target.SaveToStream(compress: true);
            SamePackage(target, merged);
            var functionMerge = request.Merges.First(merge => merge.Kind == "function");
            Reject(() => M3mScripts.Apply(target, functionMerge, "function Different() {}", library, options));
            Reject(() => M3mScripts.Apply(target, functionMerge, "invalid script", library, options));
            Reject(() => M3mScripts.Apply(target, functionMerge with { Kind = "member" }, "var int Value;", library, options));
            var memberMerge = request.Merges.First(merge => merge.Kind == "member");
            M3mScripts.Apply(target, memberMerge, "var int DeploydScriptTestValue;", library, options);
            M3mScripts.Apply(target, memberMerge,
                "function int DeploydGetScriptTestValue() { return DeploydScriptTestValue; }", library, options);
            string addedFunction = memberMerge.Entry + ".DeploydGetScriptTestValue";
            Require(target.FindExport(addedFunction)?.ClassName == "Function", "New member did not resolve the preceding variable.");
            int addedCount = target.ExportCount;
            M3mScripts.Apply(target, memberMerge,
                "function int DeploydGetScriptTestValue() { return DeploydScriptTestValue; }", library, options);
            Require(target.ExportCount == addedCount, "Reapplying a new member created duplicate exports.");
            Reject(() => M3mScripts.Apply(target, memberMerge, "var NonexistentType Value;", library, options));
            Directory.Delete(resolution, recursive: true);
        }
        finally
        {
            LegendaryExplorerCoreLibSettings.Instance.LEDirectory = previousFamilyRoot;
            LE1Directory.DefaultGamePath = previousRoot;
            foreach (var package in packages.Values)
                package.Dispose();
        }
        using var cancellation = new CancellationTokenSource();
        try
        {
            M3mScripts.Execute(request, cancellation.Token, (_, _) =>
            {
                Require(LE1Directory.DefaultGamePath is null && LegendaryExplorerCoreLibSettings.Instance.LEDirectory is null,
                    "Script compilation retained a default game location.");
                cancellation.Cancel();
            });
            throw new InvalidDataException("Cancellation after compilation was ignored.");
        }
        catch (OperationCanceledException) { }
        Require(!Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "Cancelled script compilation published output.");
        var bad = request.Scripts[0] with { Sha256 = new string('0', 64) };
        Reject(() => M3mScripts.Execute(request with { Scripts = new[] { bad }.Concat(request.Scripts.Skip(1)).ToArray() },
            CancellationToken.None, (_, _) => throw new InvalidDataException("Changed input produced progress.")));
        Console.WriteLine($"Verified {request.Merges.Length} script jobs, upstream reference, reapplication, and cancellation after compilation.");
    }

    internal static void SamePackage(IMEPackage expected, IMEPackage actual)
    {
        Require(expected.Names.SequenceEqual(actual.Names) && expected.ExportCount == actual.ExportCount
            && expected.ImportCount == actual.ImportCount, "Script package tables differ.");
        for (int index = 0; index < expected.ImportCount; index++)
            Require(expected.Imports[index].Header.AsSpan().SequenceEqual(actual.Imports[index].Header), "Script import differs.");
        for (int index = 0; index < expected.ExportCount; index++)
        {
            var before = expected.Exports[index];
            var after = actual.Exports[index];
            Require(before.InstancedFullPath == after.InstancedFullPath && before.ClassName == after.ClassName,
                "Script export identity differs.");
            if (before.ClassName == "ShaderCache")
                CommunityPatchTests.SameShaderCache(before, after);
            else
                Require(before.Data.AsSpan().SequenceEqual(after.Data), $"Script export differs: {before.InstancedFullPath}.");
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
        throw new InvalidDataException("Invalid script operation was accepted.");
    }
}
