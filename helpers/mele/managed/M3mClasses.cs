using System;
using System.Collections.Generic;
using System.IO;

using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.UnrealScript;

namespace Deployd.Mele;

internal static class M3mClasses
{
    internal static void Apply(IMEPackage target, string entry, string text, IReadOnlySet<string> originalClasses,
        FileLib library, UnrealScriptOptionsPackage options)
    {
        M3mAssets.EntryName(entry);
        string[] parts = entry.Split('.');
        if (target.Game is not (MEGame.LE1 or MEGame.LE2 or MEGame.LE3) || parts.Length > 2 || originalClasses.Contains(parts[^1]))
            throw new InvalidDataException("Class compilation requires a non-original Legendary Edition class with at most one containing package.");
        if (string.IsNullOrWhiteSpace(text) || text.Contains('\0'))
            throw new InvalidDataException("Class source is empty or contains a null character.");
        var existing = target.FindEntry(entry);
        if (existing is not null && existing is not ExportEntry { IsClass: true })
            throw new InvalidDataException("The class destination already contains an incompatible object.");
        IEntry parent = null;
        if (parts.Length == 2)
        {
            parent = target.FindEntry(parts[0]);
            if (parent is not null && parent.ClassName != "Package")
                throw new InvalidDataException("The class container is not a package.");
            if (parent is null)
            {
                var created = ExportCreator.CreatePackageExport(target, parts[0], null, cache: options.Cache);
                created.ExportFlags |= UnrealFlags.EExportFlags.ForcedExport;
                parent = created;
            }
        }
        var (compiled, log) = UnrealScriptCompiler.CompileClass(target, text, library, options,
            export: existing as ExportEntry, parent: parent, intendedClassName: parts[^1]);
        if (compiled is null || log.HasErrors || log.HasLexErrors || target.FindExport(entry) is not { IsClass: true })
            throw new InvalidDataException($"Class compilation failed for {entry}: {log}");
        if (!library.ReInitializeFile(options))
            throw new InvalidDataException($"Updated class symbols are invalid: {library.InitializationLog}");
    }
}
