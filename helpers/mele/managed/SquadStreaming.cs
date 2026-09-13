using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;

using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;

namespace Deployd.Mele;

internal static class SquadStreaming
{
    internal static int PlotId(MEGame game, string name) => (game, name) switch
    {
        (MEGame.LE2, "Convict") => 314, (MEGame.LE2, "Garrus") => 318,
        (MEGame.LE2, "Geth") => 315, (MEGame.LE2, "Grunt") => 322,
        (MEGame.LE2, "Leading") => 313, (MEGame.LE2, "Mystic") => 323,
        (MEGame.LE2, "Professor") => 321, (MEGame.LE2, "Tali") => 320,
        (MEGame.LE2, "Thief") => 317, (MEGame.LE2, "Veteran") => 324,
        (MEGame.LE2, "Vixen") => 312, (MEGame.LE2, "Assassin") => 319,
        (MEGame.LE3, "Liara") => 10152, (MEGame.LE3, "Kaidan") => 10153,
        (MEGame.LE3, "Ashley") => 10154, (MEGame.LE3, "Garrus") => 10155,
        (MEGame.LE3, "EDI") => 10156, (MEGame.LE3, "Prothean") => 10157,
        (MEGame.LE3, "Marine") => 10158, (MEGame.LE3, "Tali") => 10214,
        _ => throw new InvalidDataException("Unknown squadmate appearance plot integer."),
    };

    internal static OutfitSelection[] Apply(IMEPackage package, OutfitMerge[] outfits, bool suicide, CancellationToken cancellation)
    {
        if (package.Game is not (MEGame.LE2 or MEGame.LE3) || (suicide && package.Game != MEGame.LE2))
            throw new InvalidDataException("Squad streaming requires an LE2 or LE3 package.");
        var template = package.Exports.FirstOrDefault(entry => entry.ClassName == "LevelStreamingKismet")
            ?? throw new InvalidDataException("Squad streaming package lacks its level loader.");
        var worlds = package.Exports.Where(entry => entry.ClassName == "BioWorldInfo").ToArray();
        if (worlds.Length != 1) throw new InvalidDataException("Squad streaming package lacks an unambiguous world controller.");
        var world = worlds[0];
        var properties = world.GetProperties();
        var streaming = properties.GetProp<ArrayProperty<StructProperty>>("PlotStreaming")
            ?? throw new InvalidDataException("Squad streaming package lacks its plot table.");
        var levels = properties.GetProp<ArrayProperty<ObjectProperty>>("StreamingLevels")
            ?? throw new InvalidDataException("Squad streaming package lacks its level list.");
        var selections = new List<OutfitSelection>();
        foreach (var outfit in outfits)
        {
            cancellation.ThrowIfCancellationRequested();
            PlotId(package.Game, outfit.HenchName);
            string name = suicide ? "BioH_END_" + outfit.HenchPackage[5..] : outfit.HenchPackage;
            string chunk = (suicide ? "BioH_END_" : "BioH_") + outfit.HenchName;
            int value = Add(streaming, chunk, name, outfit.Conditional);
            var added = EntryCloner.CloneEntry(template);
            added.WriteProperty(new NameProperty(name, "PackageName"));
            if (package.Game == MEGame.LE3)
            {
                int explore = Add(streaming, chunk + "_Explore", name + "_Explore", outfit.Conditional);
                if (explore != value) throw new InvalidDataException($"Combat and exploration appearance slots differ for {outfit.HenchName}.");
                added = EntryCloner.CloneEntry(template);
                added.WriteProperty(new NameProperty(name + "_Explore", "PackageName"));
            }
            selections.Add(new OutfitSelection(outfit, value));
        }
        levels.Clear();
        foreach (var entry in package.Exports.Where(entry => entry.ClassName == "LevelStreamingKismet")) levels.Add(new ObjectProperty(entry));
        world.WriteProperties(properties);
        return selections.ToArray();
    }

    private static int Add(ArrayProperty<StructProperty> streaming, string chunk, string package, int conditional)
    {
        var matches = streaming.Where(entry => entry.GetProp<NameProperty>("VirtualChunkName")?.Value.Instanced == chunk).ToArray();
        if (matches.Length != 1) throw new InvalidDataException($"Squad streaming requires one '{chunk}' plot entry.");
        var elements = matches[0].GetProp<ArrayProperty<StructProperty>>("Elements")
            ?? throw new InvalidDataException("Squad plot entry lacks its outfit elements.");
        if (elements.Any(element => element.GetProp<NameProperty>("ChunkName")?.Value.Instanced == package))
            throw new InvalidDataException($"Squad outfit package '{package}' is already present in the streaming table.");
        int value = elements.Count;
        elements.Add(new StructProperty("PlotStreamingElement", new PropertyCollection {
            new NameProperty(package, "ChunkName"), new IntProperty(conditional, "Conditional"),
            new BoolProperty(false, "bFallback"), new NoneProperty() }));
        return value;
    }

    internal static void FixVeteran(IMEPackage package)
    {
        if (package.Game != MEGame.LE2) throw new InvalidDataException("Veteran streaming fix requires LE2.");
        var wait = package.FindExport("TheWorld.PersistentLevel.Main_Sequence.Level_Startup.SeqAct_WaitForLevelsVisible_0")
            ?? throw new InvalidDataException("Veteran mission lacks its expected level-loading condition.");
        var names = wait.GetProperty<ArrayProperty<NameProperty>>("LevelNames");
        if (names is null || names.Count == 0 || !names[0].Value.Instanced.StartsWith("BioH_Veteran", StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException("Veteran mission has an unsupported level-loading condition.");
        names[0] = new NameProperty("BioH_Veteran");
        wait.WriteProperty(names);
    }
}
