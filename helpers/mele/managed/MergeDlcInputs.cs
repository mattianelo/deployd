using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;

using LegendaryExplorerCore;
using LegendaryExplorerCore.GameFilesystem;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal.ObjectInfo;

namespace Deployd.Mele;

internal sealed record DlcRequest(int Protocol, string Operation, string Game, string GameRoot, string OriginalRoot,
    string InputRoot, string OutputRoot, InputFile[] Inputs, OutfitMerge[] Outfits, EmailMerge[] Emails, string[] Outputs);

internal sealed class MergeDlcInputs : IDisposable
{
    private readonly DlcRequest request;
    private readonly CancellationToken cancellation;
    private readonly Dictionary<string, InputFile> inputs;
    private readonly Dictionary<string, IMEPackage> packages = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, int> mounts = new(StringComparer.OrdinalIgnoreCase);
    private readonly string le1 = LE1Directory.DefaultGamePath;
    private readonly string le2 = LE2Directory.DefaultGamePath;
    private readonly string le3 = LE3Directory.DefaultGamePath;
    private readonly string family = LegendaryExplorerCoreLibSettings.Instance.LEDirectory;
    internal string Resolution { get; }
    internal MEGame Game { get; }

    internal MergeDlcInputs(DlcRequest request, CancellationToken cancellation)
    {
        this.request = request;
        this.cancellation = cancellation;
        Game = request.Game == "LE2" ? MEGame.LE2 : MEGame.LE3;
        inputs = request.Inputs.ToDictionary(input => input.Path, StringComparer.OrdinalIgnoreCase);
        Resolution = Path.Combine(request.OutputRoot, ".resolution");
        Directory.CreateDirectory(Path.Combine(Resolution, "BioGame", "CookedPCConsole"));
        Directory.CreateDirectory(Path.Combine(Resolution, "BioGame", "DLC"));
        foreach (var input in request.Inputs.Where(input => Path.GetFileName(input.Path).Equals("Mount.dlc", StringComparison.OrdinalIgnoreCase)))
        {
            cancellation.ThrowIfCancellationRequested();
            using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input, 65536);
            var data = bytes.ToArray();
            bool valid = Game == MEGame.LE2 ? data.Length >= 44 && Int(data, 0) == 684 && Int(data, 4) == 168 && Int(data, 8) == 65643
                : data.Length >= 108 && Int(data, 0) == 1 && Int(data, 4) == 685 && Int(data, 8) == 205 && Int(data, 12) == 196715;
            int mount = valid ? Int(data, Game == MEGame.LE2 ? 12 : 16) : -1;
            if (!valid || mount < 0) throw new InvalidDataException("Merge DLC requires a valid game-specific mount file.");
            if (!mounts.TryAdd(input.Path.Split('/')[1], mount)) throw new InvalidDataException("Duplicate DLC mount input.");
        }
        if (Game == MEGame.LE2) LE2UnrealObjectInfo.ObjectInfo.LoadData(null);
        else LE3UnrealObjectInfo.ObjectInfo.LoadData(null);
        LE1Directory.DefaultGamePath = null;
        LE2Directory.DefaultGamePath = null;
        LE3Directory.DefaultGamePath = null;
        LegendaryExplorerCoreLibSettings.Instance.LEDirectory = null;
    }

    private static int Int(byte[] data, int offset) => BinaryPrimitives.ReadInt32LittleEndian(data.AsSpan(offset, 4));

    internal InputFile Effective(string name, bool replacing = false)
    {
        var candidates = new List<(InputFile File, int Mount)>();
        foreach (var input in request.Inputs.Where(input => Path.GetFileName(input.Path).Equals(name, StringComparison.OrdinalIgnoreCase)))
        {
            string[] parts = input.Path.Split('/');
            int mount = 0;
            if (parts.Length == 4 && !mounts.TryGetValue(parts[1], out mount))
                throw new InvalidDataException($"Missing mount input for '{parts[1]}'.");
            candidates.Add((input, mount));
        }
        candidates.Sort((a, b) => b.Mount.CompareTo(a.Mount));
        if (candidates.Count == 0) throw new InvalidDataException($"Merge DLC requires '{name}'.");
        if (candidates.Count > 1 && candidates[0].Mount == candidates[1].Mount)
            throw new InvalidDataException($"Ambiguous DLC precedence for '{name}'.");
        if (replacing && candidates[0].Mount >= MergeDlcConfig.Mount)
            throw new InvalidDataException($"'{name}' mounts at or above the reserved merge DLC; resolve its DLC mount priority before deploying.");
        return candidates[0].File;
    }

    internal void CheckPackage(InputFile input)
    {
        cancellation.ThrowIfCancellationRequested();
        using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
        using var package = MEPackageHandler.OpenMEPackageFromStream(bytes, Path.GetFileName(input.Path), quickLoad: true);
        if (package.Game != Game) throw new InvalidDataException("Squadmate content belongs to another game.");
    }

    internal IMEPackage Open(InputFile input)
    {
        cancellation.ThrowIfCancellationRequested();
        if (!inputs.TryGetValue(input.Path, out var declared) || input != declared)
            throw new InvalidDataException("Undeclared merge DLC package input.");
        if (packages.TryGetValue(input.Path, out var existing)) return existing;
        using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
        var package = MEPackageHandler.OpenMEPackageFromStream(bytes,
            Path.Combine(Resolution, "BioGame", "CookedPCConsole", Path.GetFileName(input.Path)));
        if (package.Game != Game) { package.Dispose(); throw new InvalidDataException("Merge DLC input belongs to another game."); }
        packages.Add(input.Path, package);
        return package;
    }

    internal IMEPackage Images(OutfitMerge outfit)
    {
        string name = $"SFXHenchImages_{outfit.Dlc}.pcc";
        var input = Effective(name);
        if (!input.Path.Equals($"DLC/{outfit.Dlc}/CookedPCConsole/{name}", StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException("Squadmate images must belong to their contributing DLC.");
        return Open(input);
    }

    internal ScriptPackageCache Compiler()
    {
        var bases = LegendaryExplorerCore.UnrealScript.FileLib.BaseFileNames(Game)
            .ToDictionary(name => name, name => Open(Effective(name)), StringComparer.OrdinalIgnoreCase);
        return new ScriptPackageCache(bases, Resolution);
    }

    public void Dispose()
    {
        LE1Directory.DefaultGamePath = le1;
        LE2Directory.DefaultGamePath = le2;
        LE3Directory.DefaultGamePath = le3;
        LegendaryExplorerCoreLibSettings.Instance.LEDirectory = family;
        foreach (var package in packages.Values) package.Dispose();
        Directory.Delete(Resolution, recursive: true);
    }
}
