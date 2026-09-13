using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Xml.Linq;

using LegendaryExplorerCore.Coalesced;
using LegendaryExplorerCore.GameFilesystem;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.TLK;

namespace Deployd.Mele;

internal sealed record OutfitMerge(string Dlc, string HenchName, string HenchPackage,
    string AvailableImage, string HighlightImage, string SilhouetteImage,
    int DescriptionText, int CustomToken, int PlotFlag, int Appearance, int Conditional);
internal sealed record EmailMerge(string Dlc, string Name, int Status, string Trigger,
    int Title, int Description, int? ReadTransition, int? InMemoryBool, int Conditional, int Transition);
internal sealed record OutfitSelection(OutfitMerge Outfit, int Value);

internal sealed class MergeDlcConfig
{
    internal const string Name = "DLC_MOD_M3_MERGE";
    internal const int Mount = 45824;
    internal const int Module = 48955;
    internal const int TlkId = 1928304430;
    internal const string Cooked = "DLC/" + Name + "/CookedPCConsole/";
    internal const string Startup = "Startup_" + Name + ".pcc";
    internal static readonly string[] Languages = { "INT", "DEU", "FRA", "ITA", "ESN", "RUS", "POL", "JPN" };

    private readonly SortedDictionary<string, SortedDictionary<string, List<(string Key, string Value, int Type)>>> assets = new(StringComparer.Ordinal);

    internal void Add(string asset, string section, string key, string value, int type = 3)
    {
        if (!assets.TryGetValue(asset, out var sections)) assets.Add(asset, sections = new(StringComparer.Ordinal));
        if (!sections.TryGetValue(section, out var values)) sections.Add(section, values = new());
        values.Add((key, value, type));
    }

    internal MergeDlcConfig(MEGame game)
    {
        if (game is not (MEGame.LE2 or MEGame.LE3)) throw new InvalidDataException("Merge DLC requires LE2 or LE3.");
        if (game == MEGame.LE2)
        {
            Add("BIOEngine", "Core.System", "CookPaths", "CLEAR", 1);
            Add("BIOEngine", "Core.System", "SeekFreePCPaths", $@"..\BIOGame\DLC\{Name}\CookedPC");
            Add("BIOEngine", "Engine.DLCModules", Name, Module.ToString(), 0);
            Add("BIOEngine", "DLCInfo", "Version", "0", 0);
            Add("BIOEngine", "DLCInfo", "Flags", "0", 0);
            Add("BIOEngine", "DLCInfo", "Name", TlkId.ToString(), 0);
            Add("BIOEngine", "Engine.StartupPackages", "DLCStartupPackage", Path.GetFileNameWithoutExtension(Startup));
            Add("BIOEngine", "Engine.StartupPackages", "Package", "PlotManager" + Name);
            Add("BIOEngine", "Engine.StartupPackages", "Package", "PlotManagerAuto" + Name);
            Add("BIOGame", "SFXGame.BioWorldInfo", "ConditionalClasses", "PlotManager" + Name + ".BioAutoConditionals");
        }
    }

    internal void Outfit(MEGame game, OutfitSelection selection)
    {
        var outfit = selection.Outfit;
        string tag = "hench_" + outfit.HenchName.ToLowerInvariant();
        if (game == MEGame.LE2)
        {
            Add("BIOUI", "SFXGame.BioSFHandler_PartySelection", "lstAppearances",
                $"(Tag={tag},AddAppearance={outfit.Appearance},PlotFlag={outfit.PlotFlag})");
            return;
        }
        string silhouette = outfit.SilhouetteImage ?? "";
        Add("BIOUI", "sfxgame.sfxguidata_teamselect", "selectappearances",
            $"(AppearanceId={outfit.Appearance},MemberAppearanceValue={selection.Value},MemberTag={tag},MemberAppearancePlotLabel=Appearance{outfit.HenchName},HighlightImage=\"{outfit.HighlightImage}\",AvailableImage=\"{outfit.AvailableImage}\",DeadImage=\"GUI_Henchmen_Images.PlaceHolder\",SilhouetteImage=\"{silhouette}\",DescriptionText[0]={outfit.DescriptionText},CustomToken0[0]={outfit.CustomToken})");
        foreach (string image in new[] { outfit.AvailableImage, outfit.HighlightImage })
            Add("BIOEngine", "sfxgame.sfxengine", "dynamicloadmapping",
                $"(ObjectName=\"{image}\",SeekFreePackageName=\"SFXHenchImages_{outfit.Dlc}\")");
    }

    internal Dictionary<string, string> Xml()
    {
        var files = new Dictionary<string, string>();
        foreach (var (asset, sections) in assets)
        {
            var elements = sections.Select(section => new XElement("Section", new XAttribute("name", section.Key),
                section.Value.GroupBy(value => value.Key).Select(property => new XElement("Property", new XAttribute("name", property.Key),
                    property.Select(value => new XElement("Value", new XAttribute("type", value.Type), value.Value))))));
            files.Add(asset + ".xml", new XDocument(new XElement("CoalesceAsset", new XAttribute("id", asset),
                new XAttribute("name", asset + ".ini"), new XAttribute("source", $@"..\..\BIOGame\Config\{asset}.ini"),
                new XElement("Sections", elements))).ToString());
        }
        return files;
    }

    internal void Write(string root, MEGame game)
    {
        TransformProtocol.ValidateRoot(root);
        string directory = Path.Combine(root, Cooked);
        Directory.CreateDirectory(directory);
        TransformProtocol.ValidateRoot(directory);
        var mount = new MountFile { Game = game, MountPriority = Mount, TLKID = TlkId,
            MountFlags = game == MEGame.LE2 ? new MountFlag(0, true) : new MountFlag(EME3MountFileFlag.LoadsInSingleplayer),
            ME2Only_DLCFolderName = Name, ME2Only_DLCHumanName = "Deployd merged content" };
        using (var stream = new FileStream(Path.Combine(directory, "Mount.dlc"), FileMode.CreateNew)) mount.WriteMountFileToStream(stream);
        if (game == MEGame.LE3)
        {
            using var compiled = CoalescedConverter.CompileFromMemory(Xml());
            using var output = new FileStream(Path.Combine(directory, "Default_" + Name + ".bin"), FileMode.CreateNew);
            compiled.CopyTo(output);
        }
        else
        {
            foreach (var (asset, sections) in assets)
            {
                using var output = new StreamWriter(new FileStream(Path.Combine(directory, asset + ".ini"), FileMode.CreateNew), new UTF8Encoding(false));
                foreach (var (section, values) in sections)
                {
                    output.WriteLine($"[{section}]");
                    foreach (var value in values) output.WriteLine($"{(value.Type == 1 ? "!" : value.Type == 3 ? "+" : "")}{value.Key}={value.Value}");
                    output.WriteLine();
                }
            }
        }
        string prefix = game == MEGame.LE2 ? $"DLC_{Module}" : Name;
        foreach (string language in Languages)
        {
            string path = Path.Combine(directory, $"{prefix}_{language}.tlk");
            if (File.Exists(path)) throw new InvalidDataException("Generated merge TLK already exists.");
            var strings = new List<TLKStringRef> { new(TlkId, "Deployd merged content\0"), new(TlkId + 1, prefix + '\0'),
                new(TlkId + 2, language + '\0'), new(TlkId + 3, "Male\0"), new(TlkId + 3, "Female\0") };
            LegendaryExplorerCore.TLK.ME2ME3.HuffmanCompression.SaveToTlkFile(path, strings);
        }
    }
}
