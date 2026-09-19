using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;
using LegendaryExplorerCore.Unreal.Classes;

namespace Deployd.Mele;

internal static class TransformationTests
{
    private static int Main(string[] args)
    {
        string root = Path.Combine(Path.GetTempPath(), "deployd-transform-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        try
        {
            MEPackageHandler.Initialize();
            PackageSaver.Initialize();
            Require(typeof(MEPackageHandler).Assembly.GetManifestResourceNames().SequenceEqual(
                new[] { "LegendaryExplorerCore.Embedded.Infos.zip" }), "Unexpected resources were embedded in the helper core.");
            ParsesSupportedManifestAndRejectsUnknownSemantics();
            MergesRowsAndRestoresOriginals();
            PreservesDuplicateOriginalRows();
            RejectsWrongColumnsAndNonOriginalTables();
            RejectsInvalidRequestsAndChangedInputs(root);
            ConfigTests.Run(root);
            AssetMergeTests.Run(root);
            ScriptMergeTests.Run(root);
            OrderedM3mTests.Run(root);
            TlkPlotTests.Run(root);
            ShaderMergeTests.Run(root);
            TextureCompileTests.Run(root);
            if (args.Length == 2 && args[0] == "--shaders")
                ShaderMergeTests.Corpus(args[1]);
            else if (args.Length == 2 && args[0] == "--merge-dlc")
                MergeDlcTests.Inspect(args[1]);
            else if (args.Length == 2 && args[0] == "--community-patch-tlk")
                TlkPlotTests.VerifyTlk(args[1]);
            else if (args.Length == 2 && args[0] == "--community-patch-plot")
                PlotCorpusTests.Verify(args[1]);
            else if (args.Length == 2 && args[0] == "--community-patch")
                CommunityPatchTests.Verify(args[1]);
            else if (args.Length == 2 && args[0] == "--community-patch-config")
                ConfigTests.VerifyCorpus(args[1]);
            else if (args.Length == 2 && args[0] == "--community-patch-assets")
                AssetMergeTests.VerifyCorpus(args[1]);
            else if (args.Length == 2 && args[0] == "--community-patch-scripts")
                ScriptMergeTests.VerifyCorpus(args[1]);
            else if (args.Length == 2 && args[0] == "--community-patch-m3m")
                OrderedM3mTests.VerifyCorpus(args[1]);
            else if (args.Length == 2 && args[0] == "--community-patch-startup")
                OrderedM3mTests.VerifyStartupCorpus(args[1]);
            else if (args.Length == 1)
                RebuildsInMountOrderWithVerifiedCodec(root, args[0]);
            else if (args.Length != 0)
                throw new ArgumentException("Unexpected transformation test arguments.");
            Console.WriteLine("M3DA manifest, merge, and protocol tests passed.");
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine(error);
            return 1;
        }
        finally
        {
            Directory.Delete(root, recursive: true);
        }
    }

    private static M3daEntry[] Parse(string text)
    {
        using var stream = new MemoryStream(Encoding.UTF8.GetBytes(text));
        return M3daMerge.Parse(stream);
    }

    private static void ParsesSupportedManifestAndRejectsUnknownSemantics()
    {
        const string valid = "[{\"packagefile\":\"Engine.pcc\",\"mergepackagefile\":\"mod.pcc\",\"mergetables\":[\"Group.Values_part_1\",],},]";
        Require(Parse(valid)[0].Tables.Single() == "Group.Values_part_1", "Trailing commas were rejected.");
        Reject(() => Parse(valid.Replace("Engine.pcc", "../Engine.pcc")));
        Reject(() => Parse(valid.Replace("mod.pcc", "C:\\\\mod.pcc")));
        Reject(() => Parse(valid.Replace("Values_part_1", "Values_1")));
        Reject(() => Parse(valid.Replace("Values_part_1", "Values_part_01")));
        Reject(() => Parse(valid.Replace("Values_part_1", "Values_part_2147483647")));
        Reject(() => Parse(valid.Replace("\"packagefile\":", "\"future\":true,\"packagefile\":")));
        Reject(() => Parse(valid.Replace("\"packagefile\":", "\"packagefile\":\"SFXGame.pcc\",\"packagefile\":")));
        Reject(() => Parse("[]"));
    }

    private static IMEPackage Package(string name, int value, bool numbered = true, string column = "Value")
    {
        var package = MEPackageHandler.CreateMemoryEmptyPackage("synthetic.pcc", MEGame.LE1);
        var type = new ImportEntry(package) { ObjectName = numbered ? "Bio2DANumberedRows" : "Bio2DA", ClassName = "Class", PackageFile = "Engine" };
        package.AddImport(type);
        var export = new ExportEntry(package, 0, NameReference.FromInstancedString(name), binary: Bio2DABinary.Create()) { Class = type };
        package.AddExport(export);
        var table = new Bio2DA(export);
        table.AddColumn(column);
        table.AddRow(numbered ? "1" : "First");
        table[0, 0].IntValue = value;
        table.Write2DAToExport();
        return package;
    }

    private static void MergesRowsAndRestoresOriginals()
    {
        foreach (bool numbered in new[] { false, true })
        {
            using var original = Package("Values", 10, numbered);
            using var current = Package("Values", 999, numbered);
            using var source = Package("Values_part_1", 20, numbered);
            var added = new Bio2DA(source.Exports[0]);
            added.AddRow(numbered ? "2" : "Second");
            added[1, 0].NameValue = new NameReference("NewCellName", 3);
            added.Write2DAToExport();
            byte[] originalData = original.Exports[0].Data;
            var tables = M3daMerge.ResetTables(original, current);
            M3daMerge.Apply(source, current, "Values_part_1", tables);
            var merged = new Bio2DA(current.Exports[0]);
            Require(merged.RowCount == 2 && merged[0, 0].IntValue == 20
                && merged[1, 0].NameValue == new NameReference("NewCellName", 3), "Rows or name references did not merge.");
            M3daMerge.ResetTables(original, current);
            var reset = new Bio2DA(current.Exports[0]);
            Require(reset.RowCount == 1 && reset[0, 0].IntValue == 10, "Removal retained a previous contribution.");
            Require(originalData.SequenceEqual(original.Exports[0].Data), "Original data changed.");
        }
        using var bdts = Package("Values_part_2", 10);
        using var delta = Package("Values_part_1", 20);
        M3daMerge.Apply(delta, bdts, "Values_part_1", new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "Values_part_2" });
        Require(new Bio2DA(bdts.Exports[0])[0, 0].IntValue == 20, "BDTS table targeting failed.");
    }

    private static void RejectsWrongColumnsAndNonOriginalTables()
    {
        using var target = Package("Values", 10);
        using var wrong = Package("Values_part_1", 20, column: "Other");
        Reject(() => M3daMerge.Apply(wrong, target, "Values_part_1", new HashSet<string> { "Values" }));
        using var valid = Package("Values_part_1", 20);
        Reject(() => M3daMerge.Apply(valid, target, "Values_part_1", new HashSet<string>()));
        Reject(() => M3daMerge.Apply(valid, target, "Missing_part_1", new HashSet<string> { "Values" }));
        using var le2 = MEPackageHandler.CreateMemoryEmptyPackage("le2.pcc", MEGame.LE2);
        Reject(() => M3daMerge.ResetTables(le2, target));
    }

    private static MergeRequest Request(string root, string game)
    {
        string original = Path.Combine(root, "original");
        string input = Path.Combine(root, "input");
        string output = Path.Combine(root, Guid.NewGuid().ToString("N"));
        foreach (string directory in new[] { original, input, output, game })
            Directory.CreateDirectory(directory);
        var file = new InputFile("CookedPCConsole/Engine.pcc", 0, new string('0', 64));
        return new MergeRequest(1, "le1-m3da", game, original, input, output,
            new[] { new TargetPackage(file, file) }, Array.Empty<Contribution>());
    }

    private static void RejectsInvalidRequestsAndChangedInputs(string root)
    {
        var request = Request(root, Path.Combine(root, "game"));
        TransformProtocol.Validate(request);
        Reject(() => TransformProtocol.Validate(request with { Protocol = 2 }));
        Reject(() => TransformProtocol.Validate(request with { OutputRoot = request.InputRoot }));
        Reject(() => TransformProtocol.Validate(request with { OriginalRoot = request.InputRoot }));
        Reject(() => TransformProtocol.Validate(request with { InputRoot = request.InputRoot + "/../input" }));
        Contribution Contribution(string dlc, int mount) => new(dlc, mount,
            new InputFile($"DLC/{dlc}/CookedPCConsole/{dlc}-tables.m3da", 0, new string('0', 64)),
            new[] { new InputFile($"DLC/{dlc}/CookedPCConsole/tables.pcc", 0, new string('0', 64)) });
        var contribution = Contribution("DLC_MOD_A", 10);
        TransformProtocol.Validate(request with { Contributions = new[] { contribution } });
        Reject(() => TransformProtocol.Validate(request with { Contributions = new[] { contribution, Contribution("DLC_MOD_B", 10) } }));
        Reject(() => TransformProtocol.Validate(request with { Contributions = new[] { contribution, contribution } }));
        Reject(() => TransformProtocol.Validate(request with { Contributions = new[] { contribution with {
            Packages = new[] { contribution.Packages[0], contribution.Packages[0] } } } }));
        Reject(() => TransformProtocol.Validate(request with { Contributions = new[] { contribution with {
            Manifest = contribution.Manifest with { Path = "DLC/DLC_MOD_A/CookedPCConsole/DLC_MOD_A-.m3da" } } } }));
        string json = Path.Combine(root, "request.json");
        File.WriteAllText(json, JsonSerializer.Serialize(request, TransformProtocol.Json));
        Require(TransformProtocol.ReadRequest(json).Operation == "le1-m3da", "Valid request was rejected.");
        File.WriteAllText(json, File.ReadAllText(json).Replace("\"protocol\":1", "\"protocol\":1,\"protocol\":1"));
        Reject(() => TransformProtocol.ReadRequest(json));
        foreach (string path in new[] { "../outside", "/absolute", "C:/outside", "a\\b", "a//b", "a/./b", "a/b. " })
            Reject(() => TransformProtocol.Relative(path));
        string input = Path.Combine(request.InputRoot, "data");
        File.WriteAllText(input, "hello");
        var identity = Identify(request.InputRoot, "data");
        using (var stream = TransformProtocol.ReadVerified(request.InputRoot, identity))
            Require(stream.Length == 5, "Verified input was not read.");
        File.WriteAllText(input, "other");
        Reject(() => TransformProtocol.ReadVerified(request.InputRoot, identity).Dispose());
        File.Delete(input);
        File.CreateSymbolicLink(input, Path.Combine(root, "outside"));
        Reject(() => TransformProtocol.ReadVerified(request.InputRoot, identity).Dispose());
        File.Delete(input);
        string link = Path.Combine(root, "linked");
        Directory.CreateSymbolicLink(link, request.InputRoot);
        Reject(() => TransformProtocol.Validate(request with { InputRoot = link }));
        Directory.Delete(link);
        File.WriteAllText(Path.Combine(request.OutputRoot, "existing"), "preserve");
        Reject(() => TransformProtocol.Validate(request));
    }

    private static InputFile Identify(string root, string relative)
    {
        byte[] data = File.ReadAllBytes(Path.Combine(root, relative));
        return new InputFile(relative, data.Length, Convert.ToHexStringLower(SHA256.HashData(data)));
    }

    private static void PreservesDuplicateOriginalRows()
    {
        using var original = Package("Values", 10);
        var table = new Bio2DA(original.Exports[0]);
        table.AddRow("2");
        table[1, 0].IntValue = 20;
        table.Write2DAToExport();
        original.Exports[0].WriteProperty(new ArrayProperty<IntProperty>(
            new[] { new IntProperty(1), new IntProperty(1) }, "m_lstRowNumbers"));
        using var current = Package("Values", 999);
        using var source = Package("Values_part_1", 30);
        var originals = M3daMerge.ResetTables(original, current);
        M3daMerge.Apply(source, current, "Values_part_1", originals);
        var merged = new Bio2DA(current.Exports[0]);
        Require(merged.RowNames.SequenceEqual(new[] { "1", "1" }), "Original duplicate row IDs were lost.");
        Require(merged[0, 0].IntValue == 10 && merged[1, 0].IntValue == 30,
            "A contribution must update the last occurrence of a duplicate row ID.");
        M3daMerge.ResetTables(original, current);
        var restored = new Bio2DA(current.Exports[0]);
        Require(restored.RowNames.SequenceEqual(new[] { "1", "1" })
            && restored[0, 0].IntValue == 10 && restored[1, 0].IntValue == 20,
            "Removal did not restore both original duplicate rows.");
    }

    private static InputFile Save(IMEPackage package, string root, string relative)
    {
        string path = Path.Combine(root, relative);
        Directory.CreateDirectory(Path.GetDirectoryName(path));
        using var serialized = package.SaveToStream(compress: false);
        File.WriteAllBytes(path, serialized.ToArray());
        return Identify(root, relative);
    }

    private static void RebuildsInMountOrderWithVerifiedCodec(string root, string game)
    {
        var request = Request(Path.Combine(root, "pipeline"), game);
        using var original = Package("Values", 10);
        using var previous = Package("Values", 999);
        string path = "CookedPCConsole/Engine.pcc";
        request = request with { Targets = new[] { new TargetPackage(Save(original, request.OriginalRoot, path), Save(previous, request.InputRoot, path)) } };
        Contribution Make(string dlc, int mount, string suffix, int value)
        {
            string prefix = $"DLC/{dlc}/CookedPCConsole/";
            using var source = Package("Values_part_1", value);
            var file = Save(source, request.InputRoot, prefix + suffix + ".pcc");
            string manifest = prefix + dlc + "-" + suffix + ".m3da";
            File.WriteAllText(Path.Combine(request.InputRoot, manifest), JsonSerializer.Serialize(new[] {
                new M3daEntry("Engine.pcc", suffix + ".pcc", new[] { "Values_part_1" }) }, TransformProtocol.Json));
            return new Contribution(dlc, mount, Identify(request.InputRoot, manifest), new[] { file });
        }
        var first = Make("DLC_MOD_Z", 100, "first", 20);
        var last = Make("DLC_MOD_A", 200, "z-last", 40);
        var middle = Make("DLC_MOD_A", 200, "a-middle", 30);
        request = request with { Contributions = new[] { last, middle, first } };
        var report = M3daMerge.Execute(request, CancellationToken.None, (_, _) => { });
        using (var output = MEPackageHandler.OpenMEPackageFromStream(TransformProtocol.ReadVerified(request.OutputRoot,
            new InputFile(report[0].Path, report[0].Size, report[0].Sha256))))
            Require(new Bio2DA(output.Exports[0])[0, 0].IntValue == 40, "Mount and manifest ordering failed.");
        string rebuild = Path.Combine(root, "rebuild");
        Directory.CreateDirectory(rebuild);
        report = M3daMerge.Execute(request with { Contributions = Array.Empty<Contribution>(), OutputRoot = rebuild }, CancellationToken.None, (_, _) => { });
        using (var stream = TransformProtocol.ReadVerified(rebuild, new InputFile(report[0].Path, report[0].Size, report[0].Sha256)))
        using (var output = MEPackageHandler.OpenMEPackageFromStream(stream))
            Require(new Bio2DA(output.Exports[0])[0, 0].IntValue == 10, "Empty recipe retained previous merges.");
        string cancelled = Path.Combine(root, "cancelled");
        Directory.CreateDirectory(cancelled);
        try
        {
            M3daMerge.Execute(request with { OutputRoot = cancelled }, new CancellationToken(canceled: true), (_, _) => { });
            throw new InvalidOperationException("Cancellation was ignored.");
        }
        catch (OperationCanceledException) { }
        Require(!Directory.EnumerateFileSystemEntries(cancelled).Any(), "Cancellation published outputs.");
        foreach (var pair in request.Targets)
        {
            using var unchangedOriginal = TransformProtocol.ReadVerified(request.OriginalRoot, pair.Original);
            using var unchangedInput = TransformProtocol.ReadVerified(request.InputRoot, pair.Current);
        }
    }

    private static void Reject(Action action)
    {
        try { action(); }
        catch (Exception error) when (error is InvalidDataException or JsonException or FileNotFoundException) { return; }
        throw new InvalidOperationException("Invalid transformation input was accepted.");
    }

    private static void Require(bool condition, string message)
    {
        if (!condition)
            throw new InvalidOperationException(message);
    }
}
