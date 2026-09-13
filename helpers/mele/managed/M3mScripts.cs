using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore;
using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.GameFilesystem;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal.ObjectInfo;
using LegendaryExplorerCore.UnrealScript;
using LegendaryExplorerCore.UnrealScript.Compiling.Errors;
using LegendaryExplorerCore.UnrealScript.Language.Tree;

namespace Deployd.Mele;

internal sealed record ScriptMerge(string Entry, string Kind, string Script);
internal sealed record ScriptMergeRequest(int Protocol, string Operation, string GameRoot, string InputRoot,
    string OutputRoot, InputFile Target, InputFile[] Dependencies, InputFile[] Scripts, ScriptMerge[] Merges);

internal static class M3mScripts
{
    internal static ScriptMergeRequest ReadRequest(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (stream.Length > 4 * 1024 * 1024)
            throw new InvalidDataException("Script merge request exceeds its size limit.");
        using var document = JsonDocument.Parse(stream, new JsonDocumentOptions { MaxDepth = 16 });
        TransformProtocol.RejectDuplicateKeys(document.RootElement);
        var request = document.Deserialize<ScriptMergeRequest>(TransformProtocol.Json)
            ?? throw new InvalidDataException("A script merge request is required.");
        Validate(request);
        return request;
    }

    internal static void Validate(ScriptMergeRequest request)
    {
        if (request.Protocol != 1 || request.Operation != "le1-m3m-scripts" || request.Target is null
            || request.Dependencies is null || request.Dependencies.Length > 7
            || request.Scripts is null || request.Scripts.Length is < 1 or > 4096
            || request.Merges is null || request.Merges.Length is < 1 or > 4096)
            throw new InvalidDataException("Invalid or unsupported script merge request.");
        foreach (string root in new[] { request.GameRoot, request.InputRoot, request.OutputRoot })
            TransformProtocol.ValidateRoot(root);
        if (TransformProtocol.Overlaps(request.GameRoot, request.OutputRoot)
            || TransformProtocol.Overlaps(request.InputRoot, request.OutputRoot)
            || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any())
            throw new InvalidDataException("Script merge output must be empty and separate from all inputs.");
        string[] baseFiles = FileLib.BaseFileNames(MEGame.LE1);
        int targetIndex = Array.IndexOf(baseFiles, Path.GetFileName(request.Target.Path));
        if (targetIndex < 0 || request.Target.Path != "CookedPCConsole/" + baseFiles[targetIndex])
            throw new InvalidDataException("Script merging currently requires a canonical LE1 base package target.");
        var dependencies = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var scripts = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long total = 0;
        foreach (var input in request.Dependencies.Concat(new[] { request.Target }).Concat(request.Scripts))
        {
            if (input is null || input.Size is < 1 or > 512 * 1024 * 1024)
                throw new InvalidDataException("Invalid script merge input size.");
            TransformProtocol.Relative(input.Path);
            total = checked(total + input.Size);
            if (total > 2L * 1024 * 1024 * 1024)
                throw new InvalidDataException("Script merge inputs exceed the job size limit.");
        }
        foreach (var input in request.Dependencies)
            if (!dependencies.Add(input.Path))
                throw new InvalidDataException("Duplicate compiler dependency.");
        if (!dependencies.SetEquals(baseFiles.Take(targetIndex).Select(name => "CookedPCConsole/" + name)))
            throw new InvalidDataException("Compiler dependencies must exactly match the preceding LE1 base packages.");
        foreach (var input in request.Scripts)
            if (!input.Path.StartsWith("Scripts/", StringComparison.Ordinal) || input.Size > 4 * 1024 * 1024
                || !input.Path.EndsWith(".uc", StringComparison.OrdinalIgnoreCase) || !scripts.Add(input.Path))
                throw new InvalidDataException("Invalid or duplicate script input.");
        var used = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var merge in request.Merges)
        {
            if (merge is null || merge.Kind is not ("function" or "member") || !scripts.Contains(merge.Script))
                throw new InvalidDataException("Unsupported script operation or undeclared script input.");
            if (string.IsNullOrEmpty(merge.Entry) || merge.Entry.Length > 1024
                || merge.Entry.Split('.').Any(part => part.Length == 0
                    || part.Any(character => !char.IsAsciiLetterOrDigit(character) && character != '_')))
                throw new InvalidDataException("Invalid script target export path.");
            used.Add(merge.Script);
        }
        if (!used.SetEquals(scripts))
            throw new InvalidDataException("Script merge request contains unused scripts.");
    }

    internal static void Apply(IMEPackage target, ScriptMerge merge, string text, FileLib library,
        UnrealScriptOptionsPackage options)
    {
        var entry = target.FindExport(merge.Entry)
            ?? throw new InvalidDataException($"Script target export is missing: {merge.Entry}.");
        if (target.Game is not (MEGame.LE1 or MEGame.LE2 or MEGame.LE3) || (merge.Kind == "function" ? entry.ClassName != "Function"
            : merge.Kind != "member" || !entry.IsClass))
            throw new InvalidDataException("Script operation does not match the target export type.");
        if (string.IsNullOrWhiteSpace(text) || text.Contains('\0'))
            throw new InvalidDataException("Script source is empty or contains a null character.");
        MessageLog log;
        if (merge.Kind == "function")
        {
            log = new MessageLog();
            var (outline, _) = UnrealScriptCompiler.CompileOutlineAST(text, "Function", log, target.Game);
            if (log.HasErrors || log.HasLexErrors || outline is not Function function
                || !string.Equals(function.Name, entry.ObjectName.Instanced, StringComparison.OrdinalIgnoreCase))
                throw new InvalidDataException("Function source does not define the requested function.");
            var (compiled, result) = UnrealScriptCompiler.CompileFunction(entry, text, library, options);
            log = result;
            if (compiled is null)
                throw new InvalidDataException($"Function compilation failed for {merge.Entry}: {log}");
        }
        else
            log = UnrealScriptCompiler.AddOrReplaceInClass(entry, text, library, options);
        if (log.HasErrors || log.HasLexErrors)
            throw new InvalidDataException($"Script compilation failed for {merge.Entry}: {log}");
        if (!library.ReInitializeFile(options))
            throw new InvalidDataException($"Updated script symbols are invalid: {library.InitializationLog}");
    }

    internal static OutputFile[] Execute(ScriptMergeRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        cancellation.ThrowIfCancellationRequested();
        var scripts = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var input in request.Scripts)
        {
            cancellation.ThrowIfCancellationRequested();
            using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input, 4 * 1024 * 1024);
            try
            {
                scripts.Add(input.Path, new UTF8Encoding(false, true).GetString(bytes.ToArray()));
            }
            catch (DecoderFallbackException)
            {
                throw new InvalidDataException("Script inputs must contain valid UTF-8 source text.");
            }
        }
        if (!OodleHelper.EnsureOodleDll(request.GameRoot))
            throw new InvalidDataException("The verified game codec is unavailable.");
        LE1UnrealObjectInfo.ObjectInfo.LoadData(null);
        var packages = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        string resolutionRoot = Path.Combine(request.OutputRoot, ".resolution");
        Directory.CreateDirectory(Path.Combine(resolutionRoot, "BioGame", "CookedPCConsole"));
        Directory.CreateDirectory(Path.Combine(resolutionRoot, "BioGame", "DLC"));
        string previousRoot = LE1Directory.DefaultGamePath;
        string previousFamilyRoot = LegendaryExplorerCoreLibSettings.Instance.LEDirectory;
        LegendaryExplorerCoreLibSettings.Instance.LEDirectory = null;
        LE1Directory.DefaultGamePath = null;
        try
        {
            foreach (var input in request.Dependencies.Concat(new[] { request.Target }))
            {
                cancellation.ThrowIfCancellationRequested();
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
                string name = Path.GetFileName(input.Path);
                var package = MEPackageHandler.OpenMEPackageFromStream(bytes, Path.Combine(resolutionRoot, "BioGame", "CookedPCConsole", name));
                packages.Add(name, package);
                if (package.Game != MEGame.LE1)
                    throw new InvalidDataException("Script merge input is not an LE1 package.");
            }
            using var cache = new ScriptPackageCache(packages, resolutionRoot);
            var options = new UnrealScriptOptionsPackage
            {
                Cache = cache, GamePathOverride = resolutionRoot,
                CustomFileResolver = (name, _) => cache.ResolveCandidate(name),
            };
            var target = packages[Path.GetFileName(request.Target.Path)];
            using var library = new FileLib(target, useAutoReinitialization: false);
            if (!library.Initialize(options, canUseBinaryCache: false))
                throw new InvalidDataException($"Compiler initialization failed: {library.InitializationLog}");
            for (int index = 0; index < request.Merges.Length; index++)
            {
                cancellation.ThrowIfCancellationRequested();
                var merge = request.Merges[index];
                Apply(target, merge, scripts[merge.Script], library, options);
                progress(index + 1, request.Merges.Length);
            }
            return new[] { PackageOutput.Write(target, request.OutputRoot, request.Target.Path, cancellation) };
        }
        finally
        {
            LegendaryExplorerCoreLibSettings.Instance.LEDirectory = previousFamilyRoot;
            LE1Directory.DefaultGamePath = previousRoot;
            foreach (var package in packages.Values)
                package.Dispose();
            Directory.Delete(resolutionRoot, recursive: true);
        }
    }
}

internal sealed class ScriptPackageCache(IReadOnlyDictionary<string, IMEPackage> packages, string resolutionRoot) : PackageCache
{
    internal IMEPackage ResolveCandidate(string name)
    {
        TransformProtocol.Relative(name);
        if (name.Contains('/') || !name.EndsWith(".pcc", StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException("Invalid compiler package candidate.");
        return packages.TryGetValue(name, out var package) ? package : null;
    }

    public override IMEPackage GetCachedPackage(string packagePath, bool openIfNotInCache = true,
        Func<string, IMEPackage> openPackageMethod = null)
    {
        // Struct metadata probes the default location before its package-relative name.
        if (packagePath is null)
            return null;
        string relative = packagePath.Replace('\\', '/');
        string prefix = resolutionRoot + "/BioGame/";
        if (relative.StartsWith(prefix, StringComparison.Ordinal))
            relative = relative[prefix.Length..];
        if (relative.StartsWith("CookedPCConsole/", StringComparison.OrdinalIgnoreCase))
            relative = relative["CookedPCConsole/".Length..];
        if (relative.Contains('/') || !packages.TryGetValue(relative, out var package))
            throw new InvalidDataException("Script compilation requires an undeclared external package.");
        return package;
    }
}
