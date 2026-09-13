using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;

namespace Deployd.Mele;

internal sealed record TextureCopy(string Package, string Export, string Destination);
internal sealed record MovieEdit(InputFile Target, InputFile Movie, TextureCopy[] Images);
internal sealed record SquadUiRequest(int Protocol, string Operation, string GameRoot, string InputRoot,
    string OutputRoot, InputFile[] Assets, MovieEdit[] Movies);

internal static class SquadUi
{
    internal static SquadUiRequest ReadRequest(string path)
    {
        var request = TransformProtocol.ReadJson<SquadUiRequest>(path);
        Validate(request);
        return request;
    }

    internal static void Validate(SquadUiRequest request)
    {
        if (request.Protocol != 1 || request.Operation != "le2-squad-ui" || request.Assets is null || request.Assets.Length is < 1 or > 1024
            || request.Movies is null || request.Movies.Length is < 1 or > 4) throw new InvalidDataException("Invalid squad UI request.");
        foreach (string root in new[] { request.GameRoot, request.InputRoot, request.OutputRoot }) TransformProtocol.ValidateRoot(root);
        if (TransformProtocol.Overlaps(request.OutputRoot, request.InputRoot) || TransformProtocol.Overlaps(request.OutputRoot, request.GameRoot)
            || Directory.EnumerateFileSystemEntries(request.OutputRoot).Any()) throw new InvalidDataException("Squad UI output must be empty and separate from inputs.");
        var assets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var input in request.Assets)
        {
            if (input is null) throw new InvalidDataException("Missing squad UI image package.");
            TransformProtocol.Relative(input.Path);
            string[] parts = input.Path.Split('/');
            if (parts.Length != 4 || parts[0] != "DLC" || !MergeDlc.Identifier(parts[1]) || !parts[1].StartsWith("DLC_MOD_", StringComparison.OrdinalIgnoreCase)
                || parts[1].Equals(MergeDlcConfig.Name, StringComparison.OrdinalIgnoreCase) || parts[2] != "CookedPCConsole"
                || parts[3] != $"SFXHenchImages_{parts[1]}.pcc" || !assets.Add(input.Path)) throw new InvalidDataException("Invalid squad UI asset ownership.");
        }
        long total = 0;
        foreach (var input in request.Assets.Concat(request.Movies.SelectMany(movie => new[] { movie?.Target, movie?.Movie })))
        {
            if (input is null || input.Size is < 1 or > 512 * 1024 * 1024) throw new InvalidDataException("Invalid squad UI input size.");
            total = checked(total + input.Size);
        }
        if (total > 4L * 1024 * 1024 * 1024) throw new InvalidDataException("Squad UI inputs exceed their size limit.");
        var targets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var used = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var movie in request.Movies)
        {
            string name = Path.GetFileName(movie.Target.Path);
            if (!MergeDlc.UiPackages.Contains(name) || movie.Target.Path != MergeDlcConfig.Cooked + name || !targets.Add(name)
                || movie.Movie.Path != ".merge-ui/" + name + ".gfx" || movie.Movie.Size > 16 * 1024 * 1024
                || movie.Images is null || movie.Images.Length is < 2 or > 720) throw new InvalidDataException("Invalid squad UI edit.");
            var images = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var image in movie.Images)
            {
                if (image is null || !assets.Contains(image.Package) || !MergeDlc.Identifier(image.Export, true)
                    || image.Destination is null || !image.Destination.StartsWith("TeamSelect_I", StringComparison.Ordinal)
                    || image.Destination.Length is < 13 or > 16 || !image.Destination[12..].All(Uri.IsHexDigit)
                    || !images.Add(image.Destination)) throw new InvalidDataException("Invalid squad UI texture mapping.");
                used.Add(image.Package);
            }
        }
        if (!used.SetEquals(assets)) throw new InvalidDataException("Unused squad UI image inputs.");
    }

    internal static OutputFile[] Execute(SquadUiRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        Validate(request);
        cancellation.ThrowIfCancellationRequested();
        if (!OodleHelper.EnsureOodleDll(request.GameRoot)) throw new InvalidDataException("The verified game codec is unavailable.");
        var assets = new Dictionary<string, IMEPackage>(StringComparer.OrdinalIgnoreCase);
        string resolution = Path.Combine(request.OutputRoot, ".resolution");
        Directory.CreateDirectory(resolution);
        try
        {
            foreach (var input in request.Assets)
            {
                cancellation.ThrowIfCancellationRequested();
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, input);
                var package = MEPackageHandler.OpenMEPackageFromStream(bytes, Path.Combine(resolution, Path.GetFileName(input.Path)));
                assets.Add(input.Path, package);
                if (package.Game != MEGame.LE2) throw new InvalidDataException("Squad UI image belongs to another game.");
            }
            var outputs = new List<OutputFile>();
            foreach (var edit in request.Movies)
            {
                cancellation.ThrowIfCancellationRequested();
                using var bytes = TransformProtocol.ReadVerified(request.InputRoot, edit.Target);
                using var movie = TransformProtocol.ReadVerified(request.InputRoot, edit.Movie, 16 * 1024 * 1024);
                using var package = MEPackageHandler.OpenMEPackageFromStream(bytes, Path.Combine(resolution, Path.GetFileName(edit.Target.Path)));
                if (package.Game != MEGame.LE2) throw new InvalidDataException("Squad UI target belongs to another game.");
                var export = package.FindExport("GUI_SF_TeamSelect.TeamSelect") ?? throw new InvalidDataException("Squad UI export is missing.");
                var raw = export.GetProperty<ImmutableByteArrayProperty>("RawData") ?? throw new InvalidDataException("Squad UI movie data is missing.");
                var references = export.GetProperty<ArrayProperty<ObjectProperty>>("References") ?? throw new InvalidDataException("Squad UI references are missing.");
                var template = package.FindExport("GUI_SF_TeamSelect.TeamSelect_I1") ?? throw new InvalidDataException("Squad UI texture template is missing.");
                foreach (var image in edit.Images)
                {
                    cancellation.ThrowIfCancellationRequested();
                    string path = "GUI_SF_TeamSelect." + image.Destination;
                    if (package.FindEntry(path) is not null) throw new InvalidDataException("Generated squad UI texture collides with existing content.");
                    if (assets[image.Package].FindExport(image.Export) is not { ClassName: "Texture2D" }) throw new InvalidDataException("Squad UI source is not a texture.");
                    var added = EntryCloner.CloneEntry(template);
                    added.ObjectName = new NameReference(image.Destination);
                    M3mAssets.Apply(assets[image.Package], package,
                        new AssetMerge(edit.Target.Path, path, image.Package, image.Export, false), resolution);
                    references.Add(new ObjectProperty(added));
                }
                raw.Bytes = movie.ToArray();
                export.WriteProperty(raw);
                export.WriteProperty(references);
                outputs.Add(PackageOutput.Write(package, request.OutputRoot, edit.Target.Path, cancellation));
                progress(outputs.Count, request.Movies.Length);
            }
            return outputs.ToArray();
        }
        finally
        {
            foreach (var asset in assets.Values) asset.Dispose();
            Directory.Delete(resolution, recursive: true);
        }
    }
}
