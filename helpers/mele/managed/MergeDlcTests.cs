using System;
using System.IO;
using System.Linq;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;

namespace Deployd.Mele;

internal static class MergeDlcTests
{
    internal static void Inspect(string root)
    {
        TransformProtocol.ValidateRoot(root);
        if (!OodleHelper.EnsureOodleDll(Path.Combine(root, "game")))
            throw new InvalidDataException("Verified game codec unavailable.");
        using var package = MEPackageHandler.OpenMEPackage(Path.Combine(root, "input", "BioH_SelectGUI.pcc"), forceLoadFromDisk: true);
        if (package.Game != MEGame.LE2) throw new InvalidDataException("Squad UI requires LE2.");
        var export = package.FindExport("GUI_SF_TeamSelect.TeamSelect")
            ?? throw new InvalidDataException("Squad selection asset is missing.");
        var data = export.GetProperty<ImmutableByteArrayProperty>("RawData")
            ?? throw new InvalidDataException("Squad selection asset data is missing.");
        using (var output = new FileStream(Path.Combine(root, "ui.swf"), FileMode.CreateNew)) output.Write(data.Bytes);
        InspectMessages(root);
        InspectStartup(root);
        Console.WriteLine("Generated email sequences, startup conditionals, and LE2/LE3 merge configuration tests passed.");
    }
    private static void InspectStartup(string root)
    {
        LegendaryExplorerCore.Unreal.ObjectInfo.LE2UnrealObjectInfo.ObjectInfo.LoadData(null);
        string resolution = Path.Combine(root, "resolution");
        Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "CookedPCConsole"));
        Directory.CreateDirectory(Path.Combine(resolution, "BioGame", "DLC"));
        var packages = LegendaryExplorerCore.UnrealScript.FileLib.BaseFileNames(MEGame.LE2).ToDictionary(name => name,
            name => {
                using var bytes = new MemoryStream(File.ReadAllBytes(Path.Combine(root, "bases", name)));
                return MEPackageHandler.OpenMEPackageFromStream(bytes, Path.Combine(resolution, "BioGame", "CookedPCConsole", name));
            }, StringComparer.OrdinalIgnoreCase);
        try
        {
            using var cache = new ScriptPackageCache(packages, resolution);
            using var startup = MergeStartup.Build(resolution, cache,
                new[] { new OutfitSelection(new OutfitMerge("DLC_MOD_Test", "Vixen", "BioH_Vixen_Test", "Images.Available", "Images.Highlight", null, 0, 0, -1, 3, 10000), 3) },
                new[] { new EmailMerge("DLC_MOD_Test", "First", 900001, "return true;", 123, 124, null, null, 10001, 90000),
                    new EmailMerge("DLC_MOD_Test", "Second", 900002, "", 125, 126, null, null, 10002, 90001) },
                System.Threading.CancellationToken.None);
            PackageOutput.Write(startup, root, "startup.pcc", System.Threading.CancellationToken.None);
            using var saved = MEPackageHandler.OpenMEPackage(Path.Combine(root, "startup.pcc"), forceLoadFromDisk: true);
            var state = saved.FindExport("PlotManagerAuto" + MergeDlcConfig.Name + ".StateTransitionMap");
            if (state?.ClassName != "BioStateEventMap") throw new InvalidDataException("Startup state map is not game-readable.");
            var events = state.GetBinaryData<BioStateEventMap>();
            if (!events.StateEvents.Select(item => item.ID).SequenceEqual(new[] { 90000, 90001 })
                || !events.StateEvents.SelectMany(item => item.Elements.OfType<BioStateEventMap.BioStateEventElementInt>())
                    .Select(item => (item.GlobalInt, item.NewValue)).SequenceEqual(new[] { (900001, 1), (900002, 1) }))
                throw new InvalidDataException("Startup email transitions do not update the declared statuses.");
            foreach (int id in new[] { 10000, 10001, 10002 })
            {
                var function = saved.FindExport("PlotManager" + MergeDlcConfig.Name + ".BioAutoConditionals.F" + id);
                if (function?.ClassName != "Function" || function.GetBinaryData<UFunction>().ScriptBytes.Length == 0)
                    throw new InvalidDataException("Startup conditional has no compiled function body.");
            }
        }
        finally { foreach (var package in packages.Values) package.Dispose(); }
    }

    private static void InspectMessages(string root)
    {
        using var package = MEPackageHandler.OpenMEPackage(Path.Combine(root, "input", "BioD_Nor_103Messages.pcc"), forceLoadFromDisk: true);
        var emails = new[] {
            new EmailMerge("DLC_MOD_First", "First", 900001, "", 12345, 12346, null, null, 10000, 90000),
            new EmailMerge("DLC_MOD_Second", "Second", 900002, "return true;", 12347, 12348, 1234, 1235, 10001, 90001),
        };
        EmailGraph.Apply(package, emails, System.Threading.CancellationToken.None);
        var generated = package.Exports.Where(entry => entry.ObjectName.Instanced.StartsWith("Deployd_Email_", StringComparison.Ordinal)).ToArray();
        if (generated.Length != 6) throw new InvalidDataException("Email merge failed to add all three sequence chains.");
        PackageOutput.Write(package, root, "merged-messages.pcc", System.Threading.CancellationToken.None);
        foreach (var game in new[] { MEGame.LE2, MEGame.LE3 })
        {
            var configRoot = Path.Combine(root, "config-" + game);
            Directory.CreateDirectory(configRoot);
            var config = new MergeDlcConfig(game);
            config.Outfit(game, new OutfitSelection(new OutfitMerge("DLC_MOD_Test", game == MEGame.LE2 ? "Vixen" : "Liara",
                "BioH_Test", "Images.Available", "Images.Highlight", null, 123, 0, -1, game == MEGame.LE2 ? 3 : 255, 10000), 3));
            config.Write(configRoot, game);
            var mount = new LegendaryExplorerCore.GameFilesystem.MountFile(Path.Combine(configRoot, MergeDlcConfig.Cooked, "Mount.dlc"));
            if (mount.MountPriority != 45824 || mount.Game != game || mount.TLKID != MergeDlcConfig.TlkId)
                throw new InvalidDataException("Generated merge DLC mount differs from its approved identity.");
        }
    }
}
