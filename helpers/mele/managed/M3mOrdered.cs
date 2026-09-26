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

namespace Deployd.Mele;

internal sealed record M3mJob(string Target, string Entry, string Kind, string Input, string SourceEntry, bool AllowNew);
internal sealed record M3mRequest(int Protocol, string Operation, string Game, string GameRoot, string OriginalRoot,
    string InputRoot, string OutputRoot, TargetPackage[] Targets, InputFile[] Dependencies,
    InputFile[] Assets, InputFile[] Scripts, M3mJob[] Jobs);

internal static class M3mOrdered
{
    internal static M3mRequest ReadRequest(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (stream.Length > 4 * 1024 * 1024)
            throw new InvalidDataException("Ordered M3M request exceeds its size limit.");
        using var document = JsonDocument.Parse(stream, new JsonDocumentOptions { MaxDepth = 16 });
        TransformProtocol.RejectDuplicateKeys(document.RootElement);
        var request = document.Deserialize<M3mRequest>(TransformProtocol.Json)
            ?? throw new InvalidDataException("An ordered M3M request is required.");
        Validate(request);
        return request;
    }

    private static MEGame Game(M3mRequest request) => request.Game switch
    {
        "LE1" => MEGame.LE1,
        "LE2" => MEGame.LE2,
        "LE3" => MEGame.LE3,
        _ => throw new InvalidDataException("M3M requires an explicit Legendary Edition game."),
    };

    internal static void Validate(M3mRequest request)
    {
        var game = Game(request);
        if (request.Protocol != 1 || request.Operation != "mele-m3m-ordered"
            || request.Targets is null || request.Targets.Length < 1 || request.Targets.Length > M3mAssets.Targets(game).Length
            || request.Dependencies is null || request.Dependencies.Length > 8
            || request.Assets is null || request.Assets.Length > 1024
            || request.Scripts is null || request.Scripts.Length > 4096
            || request.Jobs is null || request.Jobs.Length is < 1 or > 4096)
            throw new InvalidDataException("Invalid or unsupported ordered M3M request.");
        foreach (string root in new[] { request.GameRoot, request.OriginalRoot, request.InputRoot, request.OutputRoot })
            TransformProtocol.ValidateRoot(root);
        if (new[] { request.GameRoot, request.OriginalRoot, request.InputRoot }.Any(root => TransformProtocol.Overlaps(root, request.OutputRoot))
            || TransformProtocol.Overlaps(request.OriginalRoot, request.InputRoot)
            || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any())
            throw new InvalidDataException("M3M outputs must be empty and separate from all inputs; originals and candidates must be separate.");
        var targets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var dependencies = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var assets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var scripts = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long total = 0;
        void Count(InputFile input, long limit = 512 * 1024 * 1024)
        {
            if (input is null || input.Size < 1 || input.Size > limit)
                throw new InvalidDataException("Invalid M3M input size.");
            TransformProtocol.Relative(input.Path);
            total = checked(total + input.Size);
            if (total > 4L * 1024 * 1024 * 1024)
                throw new InvalidDataException("M3M inputs exceed the job size limit.");
        }
        foreach (var target in request.Targets)
        {
            if (target is null) throw new InvalidDataException("Missing M3M target identity.");
            Count(target.Original);
            Count(target.Current);
            if (target.Original.Path != target.Current.Path || !targets.Add(target.Current.Path)
                || !IsTarget(game, target.Current.Path))
                throw new InvalidDataException("Invalid or duplicate M3M target.");
        }
        foreach (var input in request.Dependencies)
        {
            Count(input);
            if (!dependencies.Add(input.Path)) throw new InvalidDataException("Duplicate M3M dependency.");
        }
        foreach (var input in request.Assets)
        {
            Count(input, 128 * 1024 * 1024);
            if (!input.Path.StartsWith("Assets/", StringComparison.Ordinal)
                || !input.Path.EndsWith(".pcc", StringComparison.OrdinalIgnoreCase) || !assets.Add(input.Path))
                throw new InvalidDataException("Invalid or duplicate M3M asset.");
        }
        foreach (var input in request.Scripts)
        {
            Count(input, 4 * 1024 * 1024);
            if (!input.Path.StartsWith("Scripts/", StringComparison.Ordinal)
                || !input.Path.EndsWith(".uc", StringComparison.OrdinalIgnoreCase) || !scripts.Add(input.Path))
                throw new InvalidDataException("Invalid or duplicate M3M script.");
        }
        var usedTargets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var usedAssets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var usedScripts = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var required = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        string[] baseFiles = FileLib.BaseFileNames(game);
        foreach (var job in request.Jobs)
        {
            if (job is null || !targets.Contains(job.Target))
                throw new InvalidDataException("M3M job references an undeclared target.");
            M3mAssets.EntryName(job.Entry);
            usedTargets.Add(job.Target);
            if (job.Kind == "asset")
            {
                M3mAssets.EntryName(job.SourceEntry);
                if (!assets.Contains(job.Input)) throw new InvalidDataException("M3M job references an undeclared asset.");
                usedAssets.Add(job.Input);
            }
            else
            {
                int index = Array.IndexOf(baseFiles, Path.GetFileName(job.Target));
                if (job.Kind is not ("class" or "function" or "member") || job.SourceEntry != "" || job.AllowNew
                    || !scripts.Contains(job.Input) || index < 0 || (job.Kind == "class" && job.Entry.Split('.').Length > 2))
                    throw new InvalidDataException("Unsupported M3M script job or target.");
                usedScripts.Add(job.Input);
                required.UnionWith(baseFiles.Take(index).Select(name => "CookedPCConsole/" + name));
            }
        }
        required.ExceptWith(targets);
        if (!targets.SetEquals(usedTargets) || !assets.SetEquals(usedAssets) || !scripts.SetEquals(usedScripts)
            || !required.SetEquals(dependencies))
            throw new InvalidDataException("M3M inputs include unused files or omit required compiler dependencies.");
    }

    private static bool IsTarget(MEGame game, string path) => path.StartsWith("CookedPCConsole/", StringComparison.Ordinal)
        && path.Split('/').Length == 2 && M3mAssets.Targets(game).Contains(Path.GetFileName(path), StringComparer.Ordinal);

    internal static OutputFile[] Execute(M3mRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        var game = Game(request);
        cancellation.ThrowIfCancellationRequested();
        var scripts = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var input in request.Scripts)
        {
            cancellation.ThrowIfCancellationRequested();
            using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input, 4 * 1024 * 1024);
            try { scripts.Add(input.Path, new UTF8Encoding(false, true).GetString(bytes.ToArray())); }
            catch (DecoderFallbackException) { throw new InvalidDataException("M3M scripts must contain valid UTF-8 text."); }
        }
        if (!OodleHelper.EnsureOodleDll(request.GameRoot))
            throw new InvalidDataException("The verified game codec is unavailable.");
        if (game == MEGame.LE1) LE1UnrealObjectInfo.ObjectInfo.LoadData(null);
        else if (game == MEGame.LE2) LE2UnrealObjectInfo.ObjectInfo.LoadData(null);
        else LE3UnrealObjectInfo.ObjectInfo.LoadData(null);
        string resolution = Path.Combine(request.OutputRoot, ".resolution");
        Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "CookedPCConsole"));
        Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "DLC"));
        string previousRoot = LE1Directory.DefaultGamePath;
        string previousLe2 = LE2Directory.DefaultGamePath;
        string previousLe3 = LE3Directory.DefaultGamePath;
        string previousFamily = LegendaryExplorerCoreLibSettings.Instance.LEDirectory;
        LegendaryExplorerCoreLibSettings.Instance.LEDirectory = null;
        LE1Directory.DefaultGamePath = null;
        LE2Directory.DefaultGamePath = null;
        LE3Directory.DefaultGamePath = null;
        var packages = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        var assets = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        var originalClasses = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        try
        {
            IMEPackage Open(string root, InputFile identity)
            {
                cancellation.ThrowIfCancellationRequested();
                using var bytes = TransformProtocol.ReadVerified(root, identity);
                var package = MEPackageHandler.OpenMEPackageFromStream(bytes,
                    Path.Combine(resolution, "BioGame", "CookedPCConsole", Path.GetFileName(identity.Path)));
                if (package.Game == game) return package;
                package.Dispose();
                throw new InvalidDataException("M3M input does not match the requested game.");
            }
            foreach (var target in request.Targets)
            {
                using var original = Open(request.OriginalRoot, target.Original);
                originalClasses.UnionWith(original.Exports.Where(export => export.IsClass).Select(export => export.ObjectName.Name));
                packages.Add(Path.GetFileName(target.Current.Path), Open(request.InputRoot, target.Current));
            }
            foreach (var input in request.Dependencies)
            {
                var dependency = Open(request.InputRoot, input);
                packages.Add(Path.GetFileName(input.Path), dependency);
                originalClasses.UnionWith(dependency.Exports.Where(export => export.IsClass).Select(export => export.ObjectName.Name));
            }
            foreach (var input in request.Assets) assets.Add(input.Path, Open(request.InputRoot, input));
            using var cache = new ScriptPackageCache(packages, resolution);
            var options = new UnrealScriptOptionsPackage { Cache = cache, GamePathOverride = resolution,
                CustomFileResolver = (name, _) => cache.ResolveCandidate(name) };
            for (int index = 0; index < request.Jobs.Length; index++)
            {
                cancellation.ThrowIfCancellationRequested();
                var job = request.Jobs[index];
                var target = packages[Path.GetFileName(job.Target)];
                if (job.Kind == "asset")
                    M3mAssets.Apply(assets[job.Input], target, new AssetMerge(job.Target, job.Entry, job.Input, job.SourceEntry, job.AllowNew), resolution);
                else
                {
                    // A preceding job may change any base package used by this symbol table.
                    using var library = new FileLib(target, useAutoReinitialization: false);
                    if (!library.Initialize(options, canUseBinaryCache: false))
                        throw new InvalidDataException($"M3M compiler initialization failed: {library.InitializationLog}");
                    if (job.Kind == "class")
                        M3mClasses.Apply(target, job.Entry, scripts[job.Input], originalClasses, library, options);
                    else
                        M3mScripts.Apply(target, new ScriptMerge(job.Entry, job.Kind, job.Input), scripts[job.Input], library, options);
                }
                progress(index + 1, request.Jobs.Length);
            }
            return request.Targets.Select(target => PackageOutput.Write(packages[Path.GetFileName(target.Current.Path)],
                request.OutputRoot, target.Current.Path, cancellation)).ToArray();
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
}
