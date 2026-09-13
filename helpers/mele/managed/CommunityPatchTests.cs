using System;
using System.Buffers.Binary;
using System.IO;
using System.Linq;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal.Classes;

namespace Deployd.Mele;

internal static class CommunityPatchTests
{
    private static readonly string[] Tables =
    {
        "BIOG_2DA_GalaxyMap_X.GalaxyMap_Planet",
        "BIOG_2DA_AreaMap_X.AreaMap_AreaMap",
        "BIOG_2DA_Talents_X.Talent_TalentEffectLevels",
    };

    internal static void Verify(string root)
    {
        Require(OodleHelper.EnsureOodleDll(Path.Combine(root, "game")), "Verified game codec unavailable.");
        using var original = Open(root, "original/CookedPCConsole/Engine.pcc");
        using var source = Open(root, "input/DLC/DLC_MOD_LE1CP/CookedPCConsole/DLC_MOD_LE1CP_2DA.pcc");
        using var merged = Open(root, "merged/CookedPCConsole/Engine.pcc");
        using var reapplied = Open(root, "reapplied/CookedPCConsole/Engine.pcc");
        using var removed = Open(root, "removed/CookedPCConsole/Engine.pcc");
        foreach (string path in Tables)
        {
            var baseline = Table(original, path);
            var contribution = Table(source, path + "_part_1");
            var actual = Table(merged, path);
            VerifyMergedTable(baseline, contribution, actual, path);
            SameTable(actual, Table(reapplied, path), path + " reapplication");
        }
        foreach (var package in new[] { merged, reapplied, removed })
            VerifyOtherExports(original, package, ReferenceEquals(package, removed));
        Console.WriteLine("Real Engine.pcc: all three contributed tables, reapplication, removal, and unrelated exports verified.");
    }

    private static IMEPackage Open(string root, string relative)
    {
        using var stream = File.OpenRead(Path.Combine(root, relative));
        var package = MEPackageHandler.OpenMEPackageFromStream(stream);
        if (package.Game != MEGame.LE1)
        {
            package.Dispose();
            throw new InvalidDataException("Corpus must contain LE1 packages.");
        }
        return package;
    }

    private static Bio2DA Table(IMEPackage package, string path)
    {
        var export = package.FindExport(path);
        Require(export is not null && IsTable(export), "Missing corpus table: " + path);
        return new Bio2DA(export);
    }

    private static void VerifyMergedTable(Bio2DA original, Bio2DA contribution, Bio2DA actual, string path)
    {
        var comparer = StringComparer.OrdinalIgnoreCase;
        var originalRows = original.RowNames.ToHashSet(comparer);
        var rows = original.RowNames.Concat(contribution.RowNames.Where(row => !originalRows.Contains(row)).Distinct(comparer)).ToArray();
        Require(actual.RowNames.SequenceEqual(rows, comparer), path + ": unexpected row identities or order.");
        Require(actual.ColumnNames.SequenceEqual(original.ColumnNames, comparer), path + ": columns changed.");
        Require(contribution.ColumnNames.ToHashSet(comparer).SetEquals(original.ColumnNames), path + ": invalid corpus columns.");
        var contributed = contribution.RowNames.ToHashSet(comparer);
        var lastIndices = rows.Select((row, index) => (row, index)).GroupBy(item => item.row, comparer)
            .ToDictionary(group => group.Key, group => group.Last().index, comparer);
        int changed = 0;
        int cells = 0;
        // Vanilla talent rows include duplicate IDs; upstream resolves contributions to the last occurrence.
        for (int row = 0; row < rows.Length; row++)
            foreach (string column in original.ColumnNames)
            {
                var expected = contributed.Contains(rows[row]) && lastIndices[rows[row]] == row
                    ? contribution[rows[row], column] : original[row, column];
                Require(SameCell(expected, actual[row, column]), $"{path}: cell differs at {row}/{column}.");
                if (row >= original.RowCount || !SameCell(original[row, column], actual[row, column]))
                    changed++;
                cells++;
            }
        Require(changed > 0, path + ": corpus made no changes.");
        Console.WriteLine($"{path}: {contribution.RowCount} contributed rows, {changed} changed cells, {cells} verified cells.");
    }

    private static void VerifyOtherExports(IMEPackage original, IMEPackage actual, bool removed)
    {
        Require(original.ImportCount == actual.ImportCount && original.ExportCount == actual.ExportCount,
            "Merge changed import or export counts.");
        Require(actual.Names.Take(original.Names.Count).SequenceEqual(original.Names), "Existing name indices changed.");
        for (int index = 0; index < original.ImportCount; index++)
            Require(original.Imports[index].Header.AsSpan().SequenceEqual(actual.Imports[index].Header), "Import changed.");
        int tables = 0;
        int other = 0;
        for (int index = 0; index < original.ExportCount; index++)
        {
            var before = original.Exports[index];
            var after = actual.Exports[index];
            Require(before.InstancedFullPath == after.InstancedFullPath && before.ClassName == after.ClassName,
                "Export identity changed.");
            if (IsTable(before))
            {
                if (removed || !Tables.Contains(before.InstancedFullPath))
                    SameTable(new Bio2DA(before), new Bio2DA(after), before.InstancedFullPath);
                tables++;
            }
            else
            {
                if (before.ClassName == "ShaderCache")
                    SameShaderCache(before, after);
                else
                    Require(before.Data.AsSpan().SequenceEqual(after.Data), "Unrelated export changed: " + before.InstancedFullPath);
                other++;
            }
        }
        Console.WriteLine($"{(removed ? "Removal" : "Merge")}: {tables} table exports, {other} unrelated exports checked.");
    }

    private static bool IsTable(ExportEntry entry) => !entry.IsDefaultObject
        && entry.ClassName is "Bio2DA" or "Bio2DANumberedRows";

    internal static void SameShaderCache(ExportEntry before, ExportEntry after)
    {
        byte[] original = before.Data;
        byte[] normalized = after.Data.ToArray();
        Require(original.Length == normalized.Length && before.propsEnd() == after.propsEnd(), "Shader cache layout changed.");
        int cursor = before.propsEnd() + 1;
        int offsets = 0;
        int ReadInt()
        {
            int value = BinaryPrimitives.ReadInt32LittleEndian(original.AsSpan(cursor, 4));
            cursor = checked(cursor + 4);
            return value;
        }
        int Count()
        {
            int count = ReadInt();
            Require(count >= 0 && count <= original.Length / 4, "Invalid shader cache count.");
            return count;
        }
        void Skip(int length)
        {
            cursor = checked(cursor + length);
            Require(length >= 0 && cursor <= original.Length, "Invalid shader cache extent.");
        }
        void NextRecord()
        {
            int position = cursor;
            int end = ReadInt();
            int relative = checked(end - before.DataOffset);
            int actualEnd = BinaryPrimitives.ReadInt32LittleEndian(normalized.AsSpan(position, 4));
            Require(checked(actualEnd - after.DataOffset) == relative && relative >= cursor && relative <= original.Length,
                "Shader cache absolute offset was not relocated correctly.");
            BinaryPrimitives.WriteInt32LittleEndian(normalized.AsSpan(position, 4), end);
            cursor = relative;
            offsets++;
        }
        Skip(checked(Count() * 12));
        Skip(checked(Count() * 12));
        int shaders = Count();
        for (int index = 0; index < shaders; index++)
        {
            Skip(24);
            NextRecord();
        }
        Skip(checked(Count() * 12));
        int materials = Count();
        for (int index = 0; index < materials; index++)
        {
            Skip(16);
            Skip(checked(Count() * 32));
            Skip(checked(Count() * 44));
            Skip(checked(Count() * 29));
            Skip(8);
            NextRecord();
        }
        // Package serialization relocates absolute record offsets; every other byte must remain identical.
        Require(original.AsSpan().SequenceEqual(normalized), "Shader cache data changed beyond offset relocation.");
        Console.WriteLine($"Shader cache: {offsets} relocated offsets checked; all other bytes unchanged.");
    }

    private static void SameTable(Bio2DA expected, Bio2DA actual, string path)
    {
        Require(expected.RowNames.SequenceEqual(actual.RowNames) && expected.ColumnNames.SequenceEqual(actual.ColumnNames),
            path + ": table dimensions or ordering changed.");
        for (int row = 0; row < expected.RowCount; row++)
            for (int column = 0; column < expected.ColumnCount; column++)
                Require(SameCell(expected[row, column], actual[row, column]), $"{path}: cell differs at {row}/{column}.");
    }

    private static bool SameCell(Bio2DACell expected, Bio2DACell actual)
    {
        if (expected.Type != actual.Type)
            return false;
        return expected.Type switch
        {
            Bio2DACell.Bio2DADataType.TYPE_INT => expected.IntValue == actual.IntValue,
            Bio2DACell.Bio2DADataType.TYPE_FLOAT => BitConverter.SingleToInt32Bits(expected.FloatValue) == BitConverter.SingleToInt32Bits(actual.FloatValue),
            Bio2DACell.Bio2DADataType.TYPE_NAME => expected.NameValue.Name == actual.NameValue.Name && expected.NameValue.Number == actual.NameValue.Number,
            Bio2DACell.Bio2DADataType.TYPE_NULL => true,
            _ => throw new InvalidDataException("Unknown cell type in corpus."),
        };
    }

    private static void Require(bool condition, string message)
    {
        if (!condition)
            throw new InvalidDataException(message);
    }
}
