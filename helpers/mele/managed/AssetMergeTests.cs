using System;
using System.Buffers.Binary;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;

namespace Deployd.Mele;

internal static class AssetMergeTests
{
    internal static void Run(string root)
    {
        string resolution = Path.Combine(root, "asset-resolution");
        Directory.CreateDirectory(resolution);
        foreach (var game in new[] { MEGame.LE1, MEGame.LE2, MEGame.LE3 })
        {
            ReplacesAndRelinksDependencies(resolution, game);
            AddsNestedAssetsUnderExistingParents(resolution, game);
        }
        ComparesTextureContentAndRejectsBadOffsets();
        RejectsInvalidRequests(root);
        Console.WriteLine("M3M asset replacement, cloning, and request tests passed.");
    }

    private static ExportEntry Object(IMEPackage package, string name, int value, IEntry parent = null, string className = "Object")
    {
        var type = package.Imports.FirstOrDefault(entry => entry.ObjectName == className);
        if (type is null)
        {
            type = new ImportEntry(package) { ObjectName = className, ClassName = "Class", PackageFile = "Core" };
            package.AddImport(type);
        }
        var result = new ExportEntry(package, parent?.UIndex ?? 0, new NameReference(name),
            properties: new PropertyCollection { new IntProperty(value, "Value") }) { Class = type };
        package.AddExport(result);
        return result;
    }

    private static void ReplacesAndRelinksDependencies(string resolution, MEGame game)
    {
        using var source = MEPackageHandler.CreateMemoryEmptyPackage("asset.pcc", game);
        using var target = MEPackageHandler.CreateMemoryEmptyPackage("target.pcc", game);
        var from = Object(source, "Source", 20);
        var dependency = Object(source, "Dependency", 30);
        from.WriteProperty(new ObjectProperty(dependency.UIndex, "Reference"));
        var destination = Object(target, "Destination", 10);
        var unrelated = Object(target, "Unrelated", 99);
        byte[] before = unrelated.Data;
        byte[] sourceBefore = from.Data;
        var merge = new AssetMerge("CookedPCConsole/SFXGame.pcc", "Destination", "Assets/asset.pcc", "Source", false);
        M3mAssets.Apply(source, target, merge, resolution);
        Require(destination.GetProperty<IntProperty>("Value").Value == 20, "Asset value was not replaced.");
        int linked = destination.GetProperty<ObjectProperty>("Reference").Value;
        Require(target.GetUExport(linked).ObjectName == "Dependency", "Asset dependency was not relinked.");
        Require(unrelated.Data.AsSpan().SequenceEqual(before), "An unrelated export changed.");
        Require(from.Data.AsSpan().SequenceEqual(sourceBefore), "The source asset changed.");
        M3mAssets.Apply(source, target, merge, resolution);
        Require(target.Exports.Count(entry => entry.ObjectName == "Dependency") == 1, "Reapplication duplicated a dependency.");
        Reject(() => M3mAssets.Apply(source, target, merge with { Entry = "Missing" }, resolution));
        Reject(() => M3mAssets.Apply(source, target, merge with { SourceEntry = "Missing" }, resolution));
        using var wrongGame = MEPackageHandler.CreateMemoryEmptyPackage("wrong.pcc", game == MEGame.LE1 ? MEGame.LE2 : MEGame.LE1);
        Reject(() => M3mAssets.Apply(wrongGame, target, merge, resolution));
    }

    private static void AddsNestedAssetsUnderExistingParents(string resolution, MEGame game)
    {
        using var source = MEPackageHandler.CreateMemoryEmptyPackage("asset.pcc", game);
        using var target = MEPackageHandler.CreateMemoryEmptyPackage("target.pcc", game);
        var outer = Object(source, "Outer", 0, className: "Package");
        var inner = Object(source, "Inner", 0, outer, "Package");
        Object(source, "NewAsset", 20, inner);
        Object(source, "UnusedSibling", 90, inner);
        Object(target, "Outer", 0, className: "Package");
        var merge = new AssetMerge("CookedPCConsole/SFXGame.pcc", "Outer.Inner.NewAsset",
            "Assets/asset.pcc", "Outer.Inner.NewAsset", true);
        M3mAssets.Apply(source, target, merge, resolution);
        Require(target.FindExport(merge.Entry)?.GetProperty<IntProperty>("Value").Value == 20,
            "A nested asset was not added under the existing parent.");
        Require(target.FindExport("Outer.Inner.UnusedSibling") is null, "An unrelated sibling was imported.");
        M3mAssets.Apply(source, target, merge, resolution);
        Require(target.Exports.Count(entry => entry.InstancedFullPath == merge.Entry) == 1,
            "Reapplication duplicated a new export.");
        Reject(() => M3mAssets.Apply(source, target, merge with { Entry = "Different" }, resolution));
    }

    private static void RejectsInvalidRequests(string root)
    {
        string input = Path.Combine(root, "asset-input");
        string output = Path.Combine(root, "asset-output");
        Directory.CreateDirectory(input);
        Directory.CreateDirectory(output);
        var target = new InputFile("CookedPCConsole/SFXGame.pcc", 10, new string('0', 64));
        var asset = new InputFile("Assets/asset.pcc", 10, new string('0', 64));
        var merge = new AssetMerge(target.Path, "Object", asset.Path, "Object", false);
        var request = new AssetMergeRequest(1, "le1-m3m-assets", input, input, output, new[] { target }, new[] { asset }, new[] { merge });
        M3mAssets.Validate(request);
        Reject(() => M3mAssets.Validate(request with { Protocol = 2 }));
        Reject(() => M3mAssets.Validate(request with { Operation = "le1-m3m" }));
        Reject(() => M3mAssets.Validate(request with { OutputRoot = input }));
        Reject(() => M3mAssets.Validate(request with { Targets = new[] { target, target } }));
        Reject(() => M3mAssets.Validate(request with { Assets = new[] { asset, asset } }));
        Reject(() => M3mAssets.Validate(request with { Targets = new[] { target with { Path = "../SFXGame.pcc" } } }));
        Reject(() => M3mAssets.Validate(request with { Merges = new[] { merge with { SourceEntry = "A..B" } } }));
        Reject(() => M3mAssets.Validate(request with { Merges = Array.Empty<AssetMerge>() }));
        Reject(() => M3mAssets.Validate(request with { Assets = new[] { asset with { Size = long.MaxValue } } }));
        string json = JsonSerializer.Serialize(request, TransformProtocol.Json);
        string file = Path.Combine(root, "asset-request.json");
        foreach (string invalid in new[]
        {
            json.Replace("\"protocol\":1", "\"protocol\":1,\"protocol\":1"),
            json.Replace("\"protocol\":1", "\"protocol\":1,\"future\":true"),
            json.Replace("\"allow_new\":false", "\"allow_new\":false,\"script\":\"run\""),
        })
        {
            File.WriteAllText(file, invalid);
            Reject(() => M3mAssets.ReadRequest(file));
        }
        Directory.CreateSymbolicLink(Path.Combine(root, "asset-link"), input);
        Reject(() => M3mAssets.Validate(request with { InputRoot = Path.Combine(root, "asset-link") }));
        using var cancelled = new CancellationTokenSource();
        cancelled.Cancel();
        try { M3mAssets.Execute(request, cancelled.Token, (_, _) => { }); throw new Exception("Cancellation was ignored."); }
        catch (OperationCanceledException) { }
        Require(!Directory.EnumerateFileSystemEntries(output).Any(), "Cancelled request created output.");
    }

    private static void ComparesTextureContentAndRejectsBadOffsets()
    {
        using var source = MEPackageHandler.CreateMemoryEmptyPackage("before.pcc", MEGame.LE1);
        using var target = MEPackageHandler.CreateMemoryEmptyPackage("after.pcc", MEGame.LE1);
        var before = Object(source, "Texture", 0, className: "Texture2D");
        var after = Object(target, "Texture", 0, className: "Texture2D");
        before.DataOffset = 1024;
        after.DataOffset = 2048;
        var texture = UTexture2D.Create();
        texture.SourceArt = Array.Empty<byte>();
        texture.Mips.Add(new UTexture2D.Texture2DMipMap(new byte[] { 1, 2, 3, 4 }, 1, 1));
        before.WriteBinary(texture);
        after.WriteBinary(texture);
        SameTexture(before, after, 1024, 2048);
        byte[] original = after.Data;
        int offset = after.propsEnd() + ObjectBinary.From<UTexture2D>(after).Mips[0].MipInfoOffsetFromBinStart;
        byte[] changed = original.ToArray();
        changed[offset + 16] ^= 1;
        after.Data = changed;
        Reject(() => SameTexture(before, after, 1024, 2048));
        changed = original.ToArray();
        changed[offset + 12] ^= 1;
        after.Data = changed;
        Reject(() => SameTexture(before, after, 1024, 2048));
    }

    internal static void VerifyCorpus(string root)
    {
        var request = M3mAssets.ReadRequest(Path.Combine(root, "semantic-request.json"));
        if (!OodleHelper.EnsureOodleDll(request.GameRoot))
            throw new InvalidDataException("The verified game codec is unavailable.");
        // The production request requires an empty output; semantic inspection uses a separate root.
        foreach (var identity in request.Targets)
        {
            using var original = MEPackageHandler.OpenMEPackage(Path.Combine(root, "input", identity.Path), forceLoadFromDisk: true);
            using var merged = MEPackageHandler.OpenMEPackage(Path.Combine(root, "merged", identity.Path), forceLoadFromDisk: true);
            using var reapplied = MEPackageHandler.OpenMEPackage(Path.Combine(root, "reapplied", identity.Path), forceLoadFromDisk: true);
            Require(merged.Game == MEGame.LE1 && merged.ExportCount == reapplied.ExportCount,
                "Asset reapplication changed the export count.");
            var changed = request.Merges.Where(merge => merge.Target == identity.Path).Select(merge => merge.Entry).ToHashSet(StringComparer.OrdinalIgnoreCase);
            int matched = 0;
            foreach (var before in original.Exports)
            {
                var after = merged.FindExport(before.InstancedFullPath);
                Require(after is not null && before.ClassName == after.ClassName, "An original export was lost or changed class.");
                if (changed.Contains(before.InstancedFullPath))
                {
                    Require(!before.Data.AsSpan().SequenceEqual(after.Data), "The selected asset was not changed.");
                    matched++;
                }
                else if (before.ClassName == "ShaderCache")
                    CommunityPatchTests.SameShaderCache(before, after);
                else
                    Require(before.Data.AsSpan().SequenceEqual(after.Data), "An unrelated export changed.");
                var again = reapplied.FindExport(after.InstancedFullPath);
                if (after.ClassName == "ShaderCache")
                    CommunityPatchTests.SameShaderCache(after, again);
                else if (after.ClassName == "Texture2D" && changed.Contains(before.InstancedFullPath))
                    SameTexture(after, again, before.DataOffset, after.DataOffset);
                else
                    Require(after.Data.AsSpan().SequenceEqual(again.Data), "Reapplication changed a merged export.");
            }
            Require(matched == changed.Count, "Not all requested assets were checked.");
            using var reference = MEPackageHandler.OpenMEPackage(Path.Combine(root, "input", identity.Path), forceLoadFromDisk: true);
            foreach (var merge in request.Merges.Where(merge => merge.Target == identity.Path))
            {
                using var source = MEPackageHandler.OpenMEPackage(Path.Combine(root, "input", merge.Asset), forceLoadFromDisk: true);
                var errors = EntryImporter.ImportAndRelinkEntries(EntryImporter.PortingOption.ReplaceSingularWithRelink,
                    source.FindExport(merge.SourceEntry), reference, reference.FindExport(merge.Entry), true,
                    new RelinkerOptionsPackage { GamePathOverride = request.GameRoot, GenerateImportsForGlobalFiles = false }, out _);
                Require(errors.Count == 0, "The pinned upstream reference merge failed.");
            }
            using var serialized = reference.SaveToStream(compress: true);
            Require(reference.Names.SequenceEqual(merged.Names) && reference.ExportCount == merged.ExportCount
                && reference.ImportCount == merged.ImportCount, "Merged package tables differ from the pinned upstream reference.");
            for (int index = 0; index < reference.ExportCount; index++)
            {
                var expected = reference.Exports[index];
                var actual = merged.Exports[index];
                Require(expected.InstancedFullPath == actual.InstancedFullPath && expected.ClassName == actual.ClassName,
                    "An export differs from the pinned upstream reference.");
                if (expected.ClassName == "ShaderCache")
                    CommunityPatchTests.SameShaderCache(expected, actual);
                else
                    Require(expected.Data.AsSpan().SequenceEqual(actual.Data), "Merged data differs from the pinned upstream reference.");
            }
        }
        using var cancellation = new CancellationTokenSource();
        try
        {
            M3mAssets.Execute(request, cancellation.Token, (_, _) => cancellation.Cancel());
            throw new InvalidDataException("An interrupted asset merge reported success.");
        }
        catch (OperationCanceledException) { }
        Require(!Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "An interrupted asset merge published outputs.");
        var changedAssets = request.Assets.Select((asset, index) => index == 0
            ? asset with { Sha256 = new string('0', 64) } : asset).ToArray();
        Reject(() => M3mAssets.Execute(request with { Assets = changedAssets }, CancellationToken.None, (_, _) => { }));
        Require(!Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "A changed asset input published outputs.");
        Console.WriteLine("Community Patch asset replacements and reapplication preserve unrelated exports.");
    }

    internal static void SameTexture(ExportEntry before, ExportEntry after, int originalOffset, int previousOffset)
    {
        byte[] original = before.Data;
        byte[] normalized = after.Data.ToArray();
        Require(original.Length == normalized.Length && before.propsEnd() == after.propsEnd(), "Texture layout changed.");
        var texture = ObjectBinary.From<UTexture2D>(before);
        int start = before.propsEnd();
        void Offset(int position, bool unused = false)
        {
            int expected = BinaryPrimitives.ReadInt32LittleEndian(original.AsSpan(position, 4));
            int actual = BinaryPrimitives.ReadInt32LittleEndian(normalized.AsSpan(position, 4));
            if (unused && expected == 0 && actual == 0)
                return;
            // LE1's upstream writer retains inline texture offsets relative to the input export.
            Require(expected - originalOffset == position + 4 && actual - previousOffset == position + 4,
                "Inline texture offset does not match the input export position.");
            BinaryPrimitives.WriteInt32LittleEndian(normalized.AsSpan(position, 4), expected);
        }
        Offset(start + 12 + texture.SourceArt.Length, unused: texture.SourceArt.Length == 0);
        foreach (var mip in texture.Mips.Where(mip => mip.IsLocallyStored))
            Offset(start + mip.MipInfoOffsetFromBinStart + 12);
        Require(original.AsSpan().SequenceEqual(normalized), "Texture data changed beyond offset relocation.");
    }

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidDataException(message);
    }

    private static void Reject(Action action)
    {
        try { action(); }
        catch (Exception error) when (error is InvalidDataException or JsonException) { return; }
        throw new InvalidDataException("An invalid asset merge was accepted.");
    }
}
