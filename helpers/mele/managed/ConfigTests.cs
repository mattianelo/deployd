using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Threading;

using LegendaryExplorerCore.Coalesced;
using LegendaryExplorerCore.Coalesced.Config;
using LegendaryExplorerCore.Packages;

namespace Deployd.Mele;

internal static class ConfigTests
{
    internal static void Run(string root)
    {
        ParsesAndAppliesOrderedOperations();
        RejectsMalformedInputs();
        RebuildsInMountOrder(root);
        Console.WriteLine("LE1 M3CD operation, binary, ordering, removal, integrity, and cancellation tests passed.");
    }

    internal static Le1Config Fixture()
    {
        var config = new Le1Config();
        config.Files.Add(new ConfigFile(@"..\..\BIOGame\Config\BIOUI.ini", new List<ConfigSection>
        {
            new("Engine.UI", new List<ConfigValue> { new("List", "old"), new("Untouched", " é\tvalue ") }),
            new("Empty", new List<ConfigValue>()),
        }));
        return config;
    }

    internal static void VerifyCorpus(string root)
    {
        Le1Config Read(string name)
        {
            using var stream = File.OpenRead(Path.Combine(root, name, "CookedPCConsole/Coalesced_INT.bin"));
            return Le1Config.Read(stream);
        }
        var original = Read("original");
        var merged = Read("merged");
        Require(original.SameAs(Read("removed")), "Corpus removal did not restore the original configuration.");
        Require(merged.SameAs(Read("reapplied")), "Corpus reapplication accumulated changes.");
        var referenceInput = new Le1Config();
        // The upstream Windows parser expects host filename rules; normalize only its in-memory test input.
        referenceInput.Files.AddRange(original.Files.Select(file => new ConfigFile(file.Basename, file.Sections)));
        using var referenceBytes = referenceInput.Serialize();
        var reference = ConfigAssetBundle.FromSingleStream(MEGame.LE1, referenceBytes);
        foreach (string name in new[] { "ConfigDelta-ModSettingsMenu.m3cd", "ConfigDelta-PCOptions_Persistent.m3cd" })
        {
            string text = File.ReadAllText(Path.Combine(root, "input/DLC/DLC_MOD_LE1CP/CookedPCConsole", name));
            ConfigMerge.PerformMerge(reference, ConfigFileProxy.ParseIni(text));
        }
        var asset = merged.Files.Single(file => file.Basename.Equals("BIOUI.ini", StringComparison.OrdinalIgnoreCase));
        var touched = new[] { (Section: "Engine.BioSFManager", Key: "HandlerLibrary"),
            (Section: "ModSettings_Submenus_LE1CP.ModSettingsSubmenu_Root", Key: "menuItems") };
        foreach (var edit in touched)
        {
            var expected = reference.GetAsset("BIOUI.ini").Sections[edit.Section][edit.Key].Select(value => value.Value);
            var actual = asset.Sections.Single(section => section.Name.Equals(edit.Section, StringComparison.OrdinalIgnoreCase))
                .Values.Where(value => value.Key.Equals(edit.Key, StringComparison.OrdinalIgnoreCase)).Select(value => value.Value);
            Require(actual.SequenceEqual(expected), "Corpus differs from pinned upstream config merge: " + edit.Section);
        }
        var handlers = asset.Sections.Single(section => section.Name == "Engine.BioSFManager").Values
            .Where(value => value.Key == "HandlerLibrary").Select(value => value.Value).ToArray();
        foreach (string type in new[] { "ModHandler_LE1CPMainWheel.LE1CP_MainWheel", "ModSettingsMenu.ModHandler_ModSettingsMenu",
            "PersistentSettings.BioSFHandler_PCOptions_Persistent" })
            Require(handlers.Count(value => value.Contains(type, StringComparison.Ordinal)) == 1, "Missing or duplicated corpus handler.");
        Require(!handlers.Any(value => value.Contains("SFXGame.BioSFHandler_BrowserWheel", StringComparison.Ordinal)
            || value.Contains("SFXGame.BioSFHandler_PCOptions", StringComparison.Ordinal)), "Original handlers survived removal.");
        foreach (var config in new[] { original, merged })
        {
            var file = config.Files.Single(file => file.Basename.Equals("BIOUI.ini", StringComparison.OrdinalIgnoreCase));
            foreach (var edit in touched)
                foreach (var section in file.Sections.Where(section => section.Name.Equals(edit.Section, StringComparison.OrdinalIgnoreCase)))
                    section.Values.RemoveAll(value => value.Key.Equals(edit.Key, StringComparison.OrdinalIgnoreCase));
            file.Sections.RemoveAll(section => section.Name == touched[1].Section && section.Values.Count == 0);
        }
        Require(original.SameAs(merged), "Corpus changed unrelated configuration, localization, or whitespace.");
        Console.WriteLine("Community Patch M3CD matches pinned upstream results; three handlers, menu registration, all unrelated assets, reapplication, and removal verified.");
    }

    private static ConfigEdit[] Parse(string text)
    {
        using var stream = new MemoryStream(Encoding.UTF8.GetBytes(text));
        return M3cdMerge.Parse(stream);
    }

    private static void ParsesAndAppliesOrderedOperations()
    {
        var config = Fixture();
        using (var serialized = config.Serialize())
            Require(config.SameAs(Le1Config.Read(serialized)), "Configuration round trip changed values or empty sections.");
        M3cdMerge.Apply(config, Parse("[bioui.ini engine.ui]\n-List=old\n+List=new\n+List=new\n.List=new\nList=last\n>Other=first\n>Other=second\n!Other=ignored\n[BioUI.ini Added]\nKey=(A=1,\tB=2)"));
        var section = config.Files[0].Sections[0];
        Require(section.Values.Where(value => value.Key == "List").Select(value => value.Value).SequenceEqual(new[] { "new", "new", "last" }),
            "Add, unique-add, or removal semantics differ.");
        Require(!section.Values.Any(value => value.Key == "Other"), "Property clear failed.");
        Require(section.Values[0] == new ConfigValue("Untouched", " é\tvalue "), "Unrelated whitespace or Unicode changed.");
        Require(config.Files[0].Sections.Last().Values.Single().Value == "(A=1,\tB=2)", "Struct value or new section was lost.");
        using var output = config.Serialize();
        Require(config.SameAs(Le1Config.Read(output)), "Merged configuration did not round trip.");
    }

    private static void RejectsMalformedInputs()
    {
        foreach (string text in new[] { "", "Key=Value", "[BioUI.ini]\nKey=Value", "[../BioUI.ini Engine.UI]\nKey=Value",
            "[BioUI.ini Engine.UI]\n++Key=Value", "[BioUI.ini Engine.UI]\n?Key=Value", "[BioUI.ini Engine.UI]\n+=Value",
            "[BioUI.ini Engine.UI]\nKey=V\0alue", "[BioUI.ini Engine.UI]\nKey=V\rOther=X", "[BioUI.ini Engine.UI]\nMissing equals" })
            Reject(() => Parse(text));
        using (var invalidUtf8 = new MemoryStream(new byte[] { 0xff }))
            Reject(() => M3cdMerge.Parse(invalidUtf8));
        Reject(() => M3cdMerge.Apply(Fixture(), Parse("[Missing.ini Engine.UI]\nKey=Value")));
        foreach (byte[] bytes in new[] { Array.Empty<byte>(), BitConverter.GetBytes(-1), BitConverter.GetBytes(int.MaxValue),
            new byte[] { 1, 0, 0, 0, 0, 0, 0, 128 } })
        {
            using var stream = new MemoryStream(bytes);
            Reject(() => Le1Config.Read(stream));
        }
        using var good = Fixture().Serialize();
        using var extra = new MemoryStream(good.ToArray().Concat(new byte[] { 0 }).ToArray());
        Reject(() => Le1Config.Read(extra));
        var duplicate = Fixture();
        duplicate.Files.Add(new ConfigFile("bioui.ini", new List<ConfigSection>()));
        Reject(() => duplicate.Serialize().Dispose());
        Require(M3cdMerge.IsTarget("Coalesced_INT.bin") && !M3cdMerge.IsTarget("Engine.pcc")
            && !M3cdMerge.IsManifest("ConfigDelta-.m3cd"), "Configuration target boundary changed.");
    }

    private static InputFile Identity(string root, string path)
    {
        byte[] data = File.ReadAllBytes(Path.Combine(root, path));
        return new InputFile(path, data.Length, Convert.ToHexStringLower(SHA256.HashData(data)));
    }

    private static void RebuildsInMountOrder(string root)
    {
        string stage = Path.Combine(root, "config");
        foreach (string name in new[] { "game", "original", "input", "output", "removed", "cancelled", "invalid" })
            Directory.CreateDirectory(Path.Combine(stage, name));
        const string path = "CookedPCConsole/Coalesced_INT.bin";
        string original = Path.Combine(stage, "original"), input = Path.Combine(stage, "input");
        Directory.CreateDirectory(Path.Combine(original, "CookedPCConsole"));
        Directory.CreateDirectory(Path.Combine(input, "CookedPCConsole"));
        using (var serialized = Fixture().Serialize())
        {
            File.WriteAllBytes(Path.Combine(original, path), serialized.ToArray());
            File.WriteAllBytes(Path.Combine(input, path), serialized.ToArray());
        }
        Contribution Delta(string dlc, int mount, string suffix, string value)
        {
            string relative = $"DLC/{dlc}/CookedPCConsole/ConfigDelta-{suffix}.m3cd";
            Directory.CreateDirectory(Path.GetDirectoryName(Path.Combine(input, relative)));
            File.WriteAllText(Path.Combine(input, relative), "[BioUI.ini Engine.UI]\n>List=" + value);
            return new Contribution(dlc, mount, Identity(input, relative), Array.Empty<InputFile>());
        }
        var first = Delta("DLC_MOD_Z", 5, "z", "first");
        var last = Delta("DLC_MOD_A", 10, "z", "last");
        var middle = Delta("DLC_MOD_A", 10, "a", "middle");
        var request = new MergeRequest(1, "le1-m3cd", Path.Combine(stage, "game"), original, input,
            Path.Combine(stage, "output"), new[] { new TargetPackage(Identity(original, path), Identity(input, path)) },
            new[] { last, first, middle });
        int progress = 0;
        var report = M3cdMerge.Execute(request, CancellationToken.None, (completed, total) =>
        {
            Require(completed == ++progress && total == 3, "Incorrect configuration progress.");
        }).Single();
        using (var bytes = TransformProtocol.ReadVerified(request.OutputRoot, new InputFile(report.Path, report.Size, report.Sha256)))
            Require(Le1Config.Read(bytes).Files[0].Sections[0].Values.Last().Value == "last", "Mount or filename order failed.");
        File.Copy(Path.Combine(request.OutputRoot, path), Path.Combine(input, path), overwrite: true);
        request = request with { Targets = new[] { request.Targets[0] with { Current = Identity(input, path) } } };
        var removed = request with { OutputRoot = Path.Combine(stage, "removed"), Contributions = Array.Empty<Contribution>() };
        report = M3cdMerge.Execute(removed, CancellationToken.None, (_, _) => { }).Single();
        using (var bytes = TransformProtocol.ReadVerified(removed.OutputRoot, new InputFile(report.Path, report.Size, report.Sha256)))
            Require(Fixture().SameAs(Le1Config.Read(bytes)), "Removal retained prior config contributions.");
        try
        {
            M3cdMerge.Execute(request with { OutputRoot = Path.Combine(stage, "cancelled") }, new CancellationToken(true), (_, _) => { });
            throw new InvalidOperationException("Cancellation was ignored.");
        }
        catch (OperationCanceledException) { }
        Require(!Directory.EnumerateFiles(Path.Combine(stage, "cancelled"), "*", SearchOption.AllDirectories).Any(), "Cancellation published files.");
        var invalid = request with { OutputRoot = Path.Combine(stage, "invalid") };
        Reject(() => M3cdMerge.Execute(invalid with { Targets = new[] { request.Targets[0] with {
            Current = request.Targets[0].Current with { Sha256 = new string('0', 64) } } } }, CancellationToken.None, (_, _) => { }));
        Reject(() => TransformProtocol.Validate(invalid with { Contributions = new[] { first with { Packages = new[] { request.Targets[0].Current } } } }));
        Reject(() => TransformProtocol.Validate(invalid with { Operation = "le1-m3da" }));
        Require(!Directory.EnumerateFiles(invalid.OutputRoot, "*", SearchOption.AllDirectories).Any(), "Failure published files.");
        using var unchanged = TransformProtocol.ReadVerified(original, request.Targets[0].Original);
    }

    private static void Reject(Action action)
    {
        try { action(); }
        catch (Exception error) when (error is InvalidDataException or IOException or ArgumentException) { return; }
        throw new InvalidOperationException("Invalid configuration input was accepted.");
    }

    private static void Require(bool condition, string message)
    {
        if (!condition)
            throw new InvalidDataException(message);
    }
}
