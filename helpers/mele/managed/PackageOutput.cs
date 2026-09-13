using System;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Threading;

using LegendaryExplorerCore.Packages;

namespace Deployd.Mele;

internal static class PackageOutput
{
    internal static OutputFile Write(IMEPackage package, string root, string relative, CancellationToken cancellation)
    {
        cancellation.ThrowIfCancellationRequested();
        using var output = package.SaveToStream(compress: true);
        Validate(package, output);
        cancellation.ThrowIfCancellationRequested();
        string hash = Convert.ToHexStringLower(SHA256.HashData(output));
        output.Position = 0;
        string path = Path.Combine(root, TransformProtocol.Relative(relative));
        Directory.CreateDirectory(Path.GetDirectoryName(path));
        TransformProtocol.ValidateRoot(Path.GetDirectoryName(path));
        using var destination = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        output.CopyTo(destination);
        destination.Flush(flushToDisk: true);
        return new OutputFile(relative, output.Length, hash);
    }

    private static void Validate(IMEPackage expected, MemoryStream serialized)
    {
        serialized.Position = 0;
        using var actual = MEPackageHandler.OpenMEPackageFromStream(serialized);
        if (actual.Game != expected.Game || !actual.Names.SequenceEqual(expected.Names)
            || actual.ImportCount != expected.ImportCount || actual.ExportCount != expected.ExportCount)
            throw new InvalidDataException("Package serialization changed package tables.");
        for (int index = 0; index < expected.ImportCount; index++)
            if (!expected.Imports[index].Header.AsSpan().SequenceEqual(actual.Imports[index].Header))
                throw new InvalidDataException("Package serialization changed an import.");
        for (int index = 0; index < expected.ExportCount; index++)
        {
            var before = expected.Exports[index];
            var after = actual.Exports[index];
            if (before.InstancedFullPath != after.InstancedFullPath || before.ClassName != after.ClassName
                || !before.Data.AsSpan().SequenceEqual(after.Data))
                throw new InvalidDataException("Package serialization changed an export.");
        }
        serialized.Position = 0;
    }
}
