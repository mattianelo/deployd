using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.TLK;
using LegendaryExplorerCore.TLK.ME1;
using LegendaryExplorerCore.Unreal;

namespace Deployd.Mele;

internal static class TlkPlotTests
{
    // @variants: both
    internal static void Run(string root)
    {
        string input = Path.Combine(root, "tlk-input"), original = Path.Combine(root, "tlk-original"), output = Path.Combine(root, "tlk-output");
        foreach (string path in new[] { input, original, output }) Directory.CreateDirectory(path);
        var target = new InputFile("CookedPCConsole/Example.pcc", 4, new string('0', 64));
        var change = new TlkChange(target.Path, "tlk", new[] { new TlkString(42, "new") });
        var request = new TlkRequest(1, "le1-tlk", input, input, output, new[] { target }, new[] { change });
        TlkMerge.Validate(request);
        foreach (string name in new[] { "../Example.pcc", "../system/Example.pcc", "~docs~/Example.pcc", "Mods/Example.pcc", "CookedPCConsole/file.dll", "CookedPCConsole/a/../b.pcc", "DLC/DLC_MOD_Test/CookedPCConsole/a./b.pcc" })
            Reject(() => TlkMerge.Validate(request with { Targets = new[] { target with { Path = name } } }));
        foreach (string name in new[] { "CookedPCConsole/NPCs/Other NPCs/Example.pcc", "DLC/DLC_MOD_Test/CookedPCConsole/NPCs/Other NPCs/Example.pcc" })
            TlkMerge.Validate(request with { Targets = new[] { target with { Path = name } }, Changes = new[] { change with { Target = name } } });
        Reject(() => TlkMerge.Validate(request with { OutputRoot = input }));
        Reject(() => TlkMerge.Validate(request with { Changes = new[] { change with { Strings = new[] { new TlkString(42, "a"), new TlkString(42, "b") } } } }));
        Reject(() => TlkMerge.Validate(request with { Changes = new[] { change with { Strings = new[] { new TlkString(42, "a\0b") } } } }));
        Reject(() => TlkMerge.Validate(request with { Targets = new[] { target, target } }));
        using var cancelled = new CancellationTokenSource(); cancelled.Cancel();
        Cancelled(() => TlkMerge.Execute(request, cancelled.Token, (_, _) => throw new InvalidDataException("Cancelled TLK job progressed.")));
        using var package = MEPackageHandler.CreateMemoryEmptyPackage("Example.pcc", MEGame.LE1);
        var type = new ImportEntry(package) { ObjectName = "BioTlkFile", ClassName = "Class", PackageFile = "Engine" };
        package.AddImport(type);
        var export = new ExportEntry(package, 0, "tlk", properties: new PropertyCollection()) { Class = type };
        package.AddExport(export);
        var huffman = new HuffmanCompression();
        huffman.LoadInputData(new List<TLKStringRef> { new(43, "preserve"), new(42, "old"), new(-1, null, 0) });
        huffman.SerializeTalkfileToExport(export);
        var updates = change with { Strings = new[] { new TlkString(42, "  Café & 雪\n "), new TlkString(44, "") } };
        TlkMerge.Apply(package, updates);
        var values = new ME1TalkFile(export).StringRefs;
        Require(values.Count == 4 && values.Single(value => value.StringID == 42).Data == "  Café & 雪\n ", "TLK text changed.");
        Require(values.Single(value => value.StringID == 43).Data == "preserve" && values.Single(value => value.StringID == -1).Flags == 0, "Unrelated TLK data changed.");
        var first = new ME1TalkFile(export).StringRefs.Select(value => (value.StringID, value.Flags, value.Data)).ToArray();
        using var snapshot = package.SaveToStream(compress: false);
        snapshot.Position = 0;
        using var expectedPackage = MEPackageHandler.OpenMEPackageFromStream(snapshot, "Example.pcc");
        TlkMerge.Apply(package, updates);
        Require(first.SequenceEqual(new ME1TalkFile(export).StringRefs.Select(value => (value.StringID, value.Flags, value.Data))), "TLK reapplication changed string content.");
        SameReapplied(expectedPackage, package, new HashSet<string> { "tlk" });
        TlkMerge.Apply(package, change);
        Reject(() => SameReapplied(expectedPackage, package, new HashSet<string> { "tlk" }));
        Require(new ME1TalkFile(export).StringRefs.Single(value => value.StringID == 42).Data == "new", "Later TLK contribution did not win.");
        Reject(() => TlkMerge.Apply(package, change with { Export = "Missing" }));
        using var otherGame = MEPackageHandler.CreateMemoryEmptyPackage("Example.pcc", MEGame.LE2);
        Reject(() => TlkMerge.Apply(otherGame, change));
        var plotTarget = target with { Path = PlotMerge.Target };
        var plot = new PlotRequest(1, "le1-plot", input, original, input, output,
            new TargetPackage(plotTarget, plotTarget), Array.Empty<InputFile>(), Array.Empty<PlotContribution>());
        PlotMerge.Validate(plot);
        var firstPlot = new PlotContribution("DLC_MOD_Test", 5,
            target with { Path = "DLC/DLC_MOD_Test/CookedPCConsole/PlotManagerUpdate.pmu" });
        var secondPlot = firstPlot with { Manifest = target with { Path = "DLC/DLC_MOD_Test/CookedPCConsole/Additional.pmu" } };
        var multiple = plot with {
            Dependencies = LegendaryExplorerCore.UnrealScript.FileLib.BaseFileNames(MEGame.LE1)
                .Select(name => target with { Path = "CookedPCConsole/" + name }).ToArray(),
            Contributions = new[] { firstPlot, secondPlot }
        };
        PlotMerge.Validate(multiple);
        Reject(() => PlotMerge.Validate(multiple with { Contributions = new[] { firstPlot, firstPlot } }));
        Reject(() => PlotMerge.Validate(multiple with { Contributions = new[] { firstPlot, secondPlot with { Mount = 6 } } }));
        Reject(() => PlotMerge.Validate(multiple with { Contributions = new[] { firstPlot, secondPlot with {
            Dlc = "DLC_MOD_Other", Manifest = target with { Path = "DLC/DLC_MOD_Other/CookedPCConsole/Additional.pmu" }
        } } }));
        Reject(() => PlotMerge.Validate(multiple with { Contributions = new[] { secondPlot with {
            Manifest = target with { Path = "DLC/DLC_MOD_Test/CookedPCConsole/../Additional.pmu" }
        } } }));
        Reject(() => PlotMerge.Validate(plot with { OriginalRoot = input }));
        Reject(() => PlotMerge.Validate(plot with { Dependencies = new[] { target } }));
        Cancelled(() => PlotMerge.Execute(plot, cancelled.Token, (_, _) => throw new InvalidDataException("Cancelled plot job progressed.")));
        var functions = PlotMerge.Parse("public function bool F9(BioWorldInfo bioWorld, int Argument)\r\n{ return true; }\r\npublic function bool F9(BioWorldInfo bioWorld, int Argument)\n{ return false; }");
        Require(functions.Length == 2 && functions[0].Key == 9 && functions[1].Value.Contains("false"), "Plot declaration order changed.");
        foreach (string id in new[] { "0", "01", "-1", "2147483648", "+1", "one", "1 " }) Reject(() => PlotMerge.Parse($"public function bool F{id}() {{}}"));
        Reject(() => PlotMerge.Parse("unsupported\npublic function bool F1() {}"));
        Reject(() => PlotMerge.AddMissing(otherGame, new[] { 1 }));
        Require(!Directory.EnumerateFileSystemEntries(output).Any(), "Rejected operation published outputs.");
        Console.WriteLine("TLK Unicode, additions, ordering, flags, reapplication, and plot request tests passed.");
    }

    internal static void VerifyTlk(string root)
    {
        var request = TlkMerge.ReadRequest(Path.Combine(root, "semantic-request.json"));
        Require(OodleHelper.EnsureOodleDll(request.GameRoot), "Verified codec unavailable.");
        foreach (var input in request.Targets)
        {
            using var original = MEPackageHandler.OpenMEPackage(Path.Combine(request.InputRoot, input.Path), forceLoadFromDisk: true);
            using var merged = MEPackageHandler.OpenMEPackage(Path.Combine(root, "merged", input.Path), forceLoadFromDisk: true);
            using var reapplied = MEPackageHandler.OpenMEPackage(Path.Combine(root, "reapplied", input.Path), forceLoadFromDisk: true);
            var changes = request.Changes.Where(change => change.Target == input.Path).ToArray();
            var changed = changes.Select(change => change.Export).ToHashSet(StringComparer.OrdinalIgnoreCase);
            foreach (var entry in original.Exports)
            {
                var after = merged.FindExport(entry.InstancedFullPath);
                Require(after is not null && after.ClassName == entry.ClassName, "A TLK merge lost an original export.");
                if (!changed.Contains(entry.InstancedFullPath))
                {
                    if (entry.ClassName == "ShaderCache") CommunityPatchTests.SameShaderCache(entry, after);
                    else Require(entry.Data.AsSpan().SequenceEqual(after.Data), "An unrelated export changed in a TLK merge.");
                }
            }
            foreach (var change in changes)
            {
                var target = original.FindExport(change.Export);
                var values = new ME1TalkFile(target).StringRefs.ToList();
                foreach (var edit in change.Strings)
                {
                    var existing = values.FirstOrDefault(value => value.StringID == edit.Id);
                    if (existing is null) values.Add(new TLKStringRef(edit.Id, edit.Data));
                    else existing.Data = edit.Data;
                }
                var reference = new HuffmanCompression();
                reference.LoadInputData(values);
                reference.SerializeTalkfileToExport(target);
            }
            using var serialized = original.SaveToStream(compress: true);
            ScriptMergeTests.SamePackage(original, merged);
            SameReapplied(merged, reapplied, changed);
        }
        using var cancellation = new CancellationTokenSource();
        Cancelled(() => TlkMerge.Execute(request, cancellation.Token, (done, _) => { if (done == 1) cancellation.Cancel(); }));
        Require(!Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "Cancelled TLK merge published output.");
        bool progressed = false;
        Reject(() => TlkMerge.Execute(request with { Targets = request.Targets.Select((target, index) => index == 0
            ? target with { Sha256 = new string('0', 64) } : target).ToArray() }, CancellationToken.None, (_, _) => progressed = true));
        Require(!progressed && !Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "Changed TLK input progressed or published output.");
        Console.WriteLine($"Verified {request.Changes.Length} TLK exports in {request.Targets.Length} packages against pinned Huffman output, reapplication, and cancellation.");
    }

    private static void SameReapplied(IMEPackage first, IMEPackage again, HashSet<string> changed)
    {
        Require(first.Names.SequenceEqual(again.Names) && first.ImportCount == again.ImportCount && first.ExportCount == again.ExportCount,
            "TLK reapplication changed package tables.");
        for (int index = 0; index < first.ImportCount; index++)
            Require(first.Imports[index].Header.AsSpan().SequenceEqual(again.Imports[index].Header), "TLK reapplication changed an import.");
        for (int index = 0; index < first.ExportCount; index++)
        {
            var before = first.Exports[index];
            var after = again.Exports[index];
            Require(before.InstancedFullPath == after.InstancedFullPath && before.ClassName == after.ClassName, "TLK export identity changed.");
            if (changed.Contains(before.InstancedFullPath))
            {
                Require(before.Data.AsSpan(0, before.propsEnd()).SequenceEqual(after.Data.AsSpan(0, after.propsEnd())), "TLK export properties changed.");
                var expected = new ME1TalkFile(before).StringRefs.Select(value => (value.StringID, value.Flags, value.Data));
                var actual = new ME1TalkFile(after).StringRefs.Select(value => (value.StringID, value.Flags, value.Data));
                Require(expected.SequenceEqual(actual), "TLK reapplication changed string IDs, flags, order, or text.");
            }
            else if (before.ClassName == "ShaderCache") CommunityPatchTests.SameShaderCache(before, after);
            else Require(before.Data.AsSpan().SequenceEqual(after.Data), "TLK reapplication changed an unrelated export.");
        }
    }

    internal static void Cancelled(Action action)
    {
        try { action(); } catch (OperationCanceledException) { return; }
        throw new InvalidDataException("Cancellation was ignored.");
    }

    internal static void Reject(Action action)
    {
        try { action(); } catch (InvalidDataException) { return; }
        throw new InvalidDataException("Invalid TLK/plot operation was accepted.");
    }

    internal static void Require(bool value, string message) { if (!value) throw new InvalidDataException(message); }
}
