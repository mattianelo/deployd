using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal;

namespace Deployd.Mele;

internal static class MergeDlc
{
    internal static readonly string[] UiPackages = { "BioH_SelectGUI.pcc", "BioP_Exp1Lvl2.pcc", "BioP_Exp1Lvl3.pcc", "BioP_Exp1Lvl4.pcc" };

    internal static string[] Outputs(string game, bool outfits, bool emails)
    {
        var names = new List<string> { "Mount.dlc" };
        string prefix = game == "LE2" ? $"DLC_{MergeDlcConfig.Module}" : MergeDlcConfig.Name;
        names.AddRange(MergeDlcConfig.Languages.Select(language => $"{prefix}_{language}.tlk"));
        if (game == "LE3") names.Add("Default_" + MergeDlcConfig.Name + ".bin");
        else names.AddRange(new[] { "BIOEngine.ini", "BIOGame.ini", MergeDlcConfig.Startup });
        if (outfits)
        {
            names.Add("BioP_Global.pcc");
            if (game == "LE3") names.Add("Conditionals" + MergeDlcConfig.Name + ".cnd");
            else
            {
                names.AddRange(new[] { "BIOUI.ini", "BioP_EndGm_StuntHench.pcc", "BioD_ZyaVTL_110Jungle.pcc" });
                names.AddRange(UiPackages);
            }
        }
        if (emails) names.Add("BioD_Nor_103Messages.pcc");
        var paths = names.Select(name => MergeDlcConfig.Cooked + name).ToList();
        if (game == "LE2" && outfits) paths.AddRange(UiPackages.Select(name => ".merge-ui/" + name + ".gfx"));
        return paths.Order(StringComparer.Ordinal).ToArray();
    }

    internal static bool Identifier(string value, bool dotted = false) => !string.IsNullOrEmpty(value) && value.Length <= 255
        && value.Split('.').All(part => part.Length != 0 && part.All(ch => char.IsAsciiLetterOrDigit(ch) || ch == '_'))
        && (dotted || !value.Contains('.'));

    internal static void Validate(DlcRequest request)
    {
        if (request.Protocol != 1 || request.Operation != "mele-merge-dlc" || request.Game is not ("LE2" or "LE3")
            || request.Inputs is null || request.Inputs.Length is < 1 or > 4096 || request.Outfits is null || request.Emails is null
            || request.Outfits.Length + request.Emails.Length is < 1 or > 4096 || (request.Game == "LE3" && request.Emails.Length != 0)
            || request.Outputs is null || request.Outputs.Length > 64 || !request.Outputs.SequenceEqual(Outputs(request.Game, request.Outfits.Length != 0, request.Emails.Length != 0)))
            throw new InvalidDataException("Invalid merge DLC request or output inventory.");
        foreach (string root in new[] { request.GameRoot, request.OriginalRoot, request.InputRoot, request.OutputRoot }) TransformProtocol.ValidateRoot(root);
        if (new[] { request.GameRoot, request.OriginalRoot, request.InputRoot }.Any(root => TransformProtocol.Overlaps(root, request.OutputRoot))
            || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any()) throw new InvalidDataException("Merge DLC output must be empty and separate from its inputs.");
        var paths = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long total = 0;
        foreach (var input in request.Inputs)
        {
            if (input is null || input.Size is < 1 or > 512 * 1024 * 1024 || !paths.Add(input.Path)) throw new InvalidDataException("Invalid or duplicate merge DLC input.");
            TransformProtocol.Relative(input.Path);
            string[] parts = input.Path.Split('/');
            bool cooked = parts.Length == 2 && parts[0].Equals("CookedPCConsole", StringComparison.OrdinalIgnoreCase);
            bool dlc = parts.Length == 4 && parts[0].Equals("DLC", StringComparison.OrdinalIgnoreCase) && Identifier(parts[1])
                && !parts[1].Equals(MergeDlcConfig.Name, StringComparison.OrdinalIgnoreCase) && parts[2].Equals("CookedPCConsole", StringComparison.OrdinalIgnoreCase);
            if (!(cooked || dlc) || !(input.Path.EndsWith(".pcc", StringComparison.OrdinalIgnoreCase)
                || (dlc && parts[3].Equals("Mount.dlc", StringComparison.OrdinalIgnoreCase) && input.Size <= 65536)))
                throw new InvalidDataException("Unsupported merge DLC input path.");
            total = checked(total + input.Size);
        }
        if (total > 4L * 1024 * 1024 * 1024) throw new InvalidDataException("Merge DLC inputs exceed their size limit.");
        var conditionals = new HashSet<int>();
        var slots = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var outfit in request.Outfits)
        {
            if (outfit is null || !Identifier(outfit.Dlc) || !outfit.Dlc.StartsWith("DLC_MOD_", StringComparison.OrdinalIgnoreCase)
                || !Identifier(outfit.HenchName) || !Identifier(outfit.HenchPackage) || !outfit.HenchPackage.StartsWith("BioH_", StringComparison.Ordinal)
                || !Identifier(outfit.AvailableImage, true) || !(string.IsNullOrEmpty(outfit.HighlightImage) && request.Game == "LE3" || Identifier(outfit.HighlightImage, true))
                || !(outfit.SilhouetteImage is null || Identifier(outfit.SilhouetteImage, true)) || outfit.PlotFlag < -1
                || outfit.Conditional is < 10000 or > 14095 || !conditionals.Add(outfit.Conditional)
                || (request.Game == "LE2" ? outfit.Appearance is < 2 or > 31 : outfit.Appearance is < 255 or > 1278)
                || !slots.Add($"{outfit.HenchName}:{outfit.Appearance}")) throw new InvalidDataException("Invalid squadmate merge contribution.");
        }
        var statuses = new HashSet<int>();
        var transitions = new HashSet<int>();
        foreach (var email in request.Emails)
        {
            if (email is null || !Identifier(email.Dlc) || email.Name is null || email.Name.Length > 1024
                || email.Trigger is null || email.Trigger.Length > 65536 || email.Trigger.Contains('\0')
                || email.Status < 0 || !statuses.Add(email.Status) || email.InMemoryBool < 0 || email.ReadTransition < 0
                || email.Conditional is < 10000 or > 14095 || !conditionals.Add(email.Conditional)
                || email.Transition is < 90000 or > 94095 || !transitions.Add(email.Transition)) throw new InvalidDataException("Invalid email merge contribution.");
        }
    }

    internal static DlcRequest ReadRequest(string path)
    {
        var request = TransformProtocol.ReadJson<DlcRequest>(path);
        Validate(request);
        return request;
    }

    internal static OutputFile[] Execute(DlcRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        cancellation.ThrowIfCancellationRequested();
        if (!OodleHelper.EnsureOodleDll(request.GameRoot)) throw new InvalidDataException("The verified game codec is unavailable.");
        using (var inputs = new MergeDlcInputs(request, cancellation))
        {
            progress(1, 7);
            foreach (var outfit in request.Outfits)
            {
                cancellation.ThrowIfCancellationRequested();
                inputs.CheckPackage(inputs.Effective(outfit.HenchPackage + ".pcc"));
                if (inputs.Game == MEGame.LE3) inputs.CheckPackage(inputs.Effective(outfit.HenchPackage + "_Explore.pcc"));
                else inputs.CheckPackage(inputs.Effective("BioH_END_" + outfit.HenchPackage[5..] + ".pcc"));
                var images = inputs.Images(outfit);
                foreach (string name in new[] { outfit.AvailableImage, outfit.HighlightImage }.Where(name => !string.IsNullOrEmpty(name)))
                    if (images.FindExport(name) is not { ClassName: "Texture2D" }) throw new InvalidDataException($"Squadmate image '{name}' is missing or is not a texture.");
            }
            progress(2, 7);
            OutfitSelection[] selections = Array.Empty<OutfitSelection>();
            if (request.Outfits.Length != 0)
            {
                var global = inputs.Open(inputs.Effective("BioP_Global.pcc", replacing: true));
                selections = SquadStreaming.Apply(global, request.Outfits, false, cancellation);
                PackageOutput.Write(global, request.OutputRoot, MergeDlcConfig.Cooked + "BioP_Global.pcc", cancellation);
                if (inputs.Game == MEGame.LE2)
                {
                    var suicide = inputs.Open(inputs.Effective("BioP_EndGm_StuntHench.pcc", replacing: true));
                    var end = SquadStreaming.Apply(suicide, request.Outfits, true, cancellation);
                    if (!end.Select(item => item.Value).SequenceEqual(selections.Select(item => item.Value)))
                        throw new InvalidDataException("Normal and suicide-mission outfit slots are inconsistent.");
                    PackageOutput.Write(suicide, request.OutputRoot, MergeDlcConfig.Cooked + "BioP_EndGm_StuntHench.pcc", cancellation);
                }
                else
                {
                    var conditionals = new CNDFile { ConditionalEntries = selections.Select(selection => new CNDFile.ConditionalEntry {
                        ID = selection.Outfit.Conditional,
                        Data = ME3ConditionalsCompiler.Compile($"(plot.ints[{SquadStreaming.PlotId(inputs.Game, selection.Outfit.HenchName)}] == i{selection.Value})") }).ToList() };
                    conditionals.ToFile(Path.Combine(request.OutputRoot, MergeDlcConfig.Cooked, "Conditionals" + MergeDlcConfig.Name + ".cnd"));
                }
            }
            progress(3, 7);
            if (inputs.Game == MEGame.LE2)
            {
                using var cache = inputs.Compiler();
                using var startup = MergeStartup.Build(inputs.Resolution, cache, selections, request.Emails, cancellation);
                PackageOutput.Write(startup, request.OutputRoot, MergeDlcConfig.Cooked + MergeDlcConfig.Startup, cancellation);
            }
            progress(4, 7);
            if (request.Emails.Length != 0)
            {
                var messages = inputs.Open(inputs.Effective("BioD_Nor_103Messages.pcc", replacing: true));
                EmailGraph.Apply(messages, request.Emails, cancellation);
                PackageOutput.Write(messages, request.OutputRoot, MergeDlcConfig.Cooked + "BioD_Nor_103Messages.pcc", cancellation);
            }
            progress(5, 7);
            if (inputs.Game == MEGame.LE2 && request.Outfits.Length != 0)
            {
                Directory.CreateDirectory(Path.Combine(request.OutputRoot, ".merge-ui"));
                foreach (string name in UiPackages)
                {
                    var package = inputs.Open(inputs.Effective(name, replacing: true));
                    var movie = package.FindExport("GUI_SF_TeamSelect.TeamSelect")?.GetProperty<ImmutableByteArrayProperty>("RawData")
                        ?? throw new InvalidDataException("Squad UI package lacks its game-owned movie.");
                    if (movie.Bytes.Length is < 13 or > 16 * 1024 * 1024) throw new InvalidDataException("Squad UI movie exceeds its size limit.");
                    using (var output = new FileStream(Path.Combine(request.OutputRoot, ".merge-ui", name + ".gfx"), FileMode.CreateNew)) output.Write(movie.Bytes);
                    PackageOutput.Write(package, request.OutputRoot, MergeDlcConfig.Cooked + name, cancellation);
                }
                var veteran = inputs.Open(inputs.Effective("BioD_ZyaVTL_110Jungle.pcc", replacing: true));
                SquadStreaming.FixVeteran(veteran);
                PackageOutput.Write(veteran, request.OutputRoot, MergeDlcConfig.Cooked + "BioD_ZyaVTL_110Jungle.pcc", cancellation);
            }
            progress(6, 7);
            var config = new MergeDlcConfig(inputs.Game);
            foreach (var selection in selections) config.Outfit(inputs.Game, selection);
            config.Write(request.OutputRoot, inputs.Game);
            cancellation.ThrowIfCancellationRequested();
            progress(7, 7);
        }
        return request.Outputs.Select(path => {
            using var file = new FileStream(Path.Combine(request.OutputRoot, path), FileMode.Open, FileAccess.Read, FileShare.Read);
            return new OutputFile(path, file.Length, Convert.ToHexStringLower(SHA256.HashData(file)));
        }).ToArray();
    }
}
