using System;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text.Json;
using System.Threading;

using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Packages;

namespace Deployd.Mele;

internal static class Program
{
    private static int Main(string[] args)
    {
        try
        {
            if (args.Length == 1 && args[0] == "capabilities")
            {
                Console.WriteLine(JsonSerializer.Serialize(new
                {
                    protocol = 1,
                    backend = "deployd-mele",
                    version = "0.11.0",
                    games = new[] { "LE1", "LE2", "LE3" },
                    capabilities = new[] { "le1-m3da", "le1-m3cd", "le1-m3m-assets", "le1-m3m-scripts", "mele-m3m-ordered", "le1-tlk", "le1-plot", "mele-plot", "mele-merge-dlc", "le2-squad-ui", "mele-m3gs" },
                    validation = new[] { "package-roundtrip" },
                }));
                return 0;
            }
            if (args.Length == 2 && args[0] is "transform" or "transform-assets" or "transform-scripts" or "transform-m3m" or "transform-tlk" or "transform-plot" or "transform-dlc" or "transform-squad-ui" or "transform-shaders")
            {
                using var cancellation = new CancellationTokenSource();
                Console.CancelKeyPress += (_, signal) => { signal.Cancel = true; cancellation.Cancel(); };
                MEPackageHandler.Initialize();
                PackageSaver.Initialize();
                void Progress(int completed, int total) =>
                    Console.WriteLine(JsonSerializer.Serialize(new { protocol = 1, type = "progress", completed, total }));
                OutputFile[] outputs;
                if (args[0] == "transform-shaders")
                    outputs = ShaderMerge.Execute(ShaderMerge.ReadRequest(args[1]), cancellation.Token, Progress);
                else if (args[0] == "transform-dlc")
                    outputs = MergeDlc.Execute(MergeDlc.ReadRequest(args[1]), cancellation.Token, Progress);
                else if (args[0] == "transform-squad-ui")
                    outputs = SquadUi.Execute(SquadUi.ReadRequest(args[1]), cancellation.Token, Progress);
                else if (args[0] == "transform-tlk")
                    outputs = TlkMerge.Execute(TlkMerge.ReadRequest(args[1]), cancellation.Token, Progress);
                else if (args[0] == "transform-plot")
                    outputs = PlotMerge.Execute(PlotMerge.ReadRequest(args[1]), cancellation.Token, Progress);
                else if (args[0] == "transform-m3m")
                    outputs = M3mOrdered.Execute(M3mOrdered.ReadRequest(args[1]), cancellation.Token, Progress);
                else if (args[0] == "transform-scripts")
                    outputs = M3mScripts.Execute(M3mScripts.ReadRequest(args[1]), cancellation.Token, Progress);
                else if (args[0] == "transform-assets")
                    outputs = M3mAssets.Execute(M3mAssets.ReadRequest(args[1]), cancellation.Token, Progress);
                else
                {
                    var request = TransformProtocol.ReadRequest(args[1]);
                    outputs = request.Operation == "le1-m3cd"
                        ? M3cdMerge.Execute(request, cancellation.Token, Progress)
                        : M3daMerge.Execute(request, cancellation.Token, Progress);
                }
                cancellation.Token.ThrowIfCancellationRequested();
                Console.WriteLine(JsonSerializer.Serialize(new { protocol = 1, type = "complete", outputs }, TransformProtocol.Json));
                return 0;
            }
            if (args.Length != 4 || args[0] != "verify-roundtrip")
                throw new ArgumentException("Expected capabilities, transform <request>, transform-assets <request>, transform-scripts <request>, transform-m3m <request>, transform-tlk <request>, transform-plot <request>, transform-dlc <request>, transform-squad-ui <request>, transform-shaders <request>, or verify-roundtrip <game-root> <input> <new-output>.");
            RoundTrip(args[1], args[2], args[3]);
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine(JsonSerializer.Serialize(new
            {
                protocol = 1, type = "error", code = error.GetType().Name,
                message = error is InvalidDataException or ArgumentException ? error.Message
                    : "Helper transformation failed; discard its outputs. No game deployment was performed.",
                at = new StackTrace(error).GetFrames().Select(frame => frame.GetMethod()?.Name).ToArray(),
            }));
            return 1;
        }
    }

    private static void RoundTrip(string gameRoot, string input, string output)
    {
        // This diagnostic only publishes a new file; it is not a deployment operation.
        using var source = new FileStream(input, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (File.Exists(output) || Directory.Exists(output))
            throw new IOException("The diagnostic output must be a new file.");
        if (!OodleHelper.EnsureOodleDll(gameRoot))
            throw new InvalidOperationException("The verified game codec is unavailable.");
        MEPackageHandler.Initialize();
        PackageSaver.Initialize();
        using IMEPackage package = MEPackageHandler.OpenMEPackageFromStream(source);
        if (package.Game is not (MEGame.LE1 or MEGame.LE2 or MEGame.LE3))
            throw new InvalidDataException("Only Legendary Edition packages are supported.");
        if (package is not MEPackage mePackage)
            throw new InvalidDataException("Unexpected package implementation.");
        var importHeaders = package.Imports.Select(entry => entry.Header).ToArray();
        var exports = package.Exports.Select(entry => new
        {
            entry.InstancedFullPath, entry.ClassName, data = SHA256.HashData(entry.Data),
        }).ToArray();
        using var serialized = mePackage.SaveToStream(compress: true);
        serialized.Position = 0;
        using IMEPackage reopened = MEPackageHandler.OpenMEPackageFromStream(serialized);
        if (package.Game != reopened.Game || !package.Names.SequenceEqual(reopened.Names)
            || package.ImportCount != reopened.ImportCount || package.ExportCount != reopened.ExportCount)
            throw new InvalidDataException("Package tables changed during serialization.");
        for (int index = 0; index < package.ImportCount; index++)
            if (!importHeaders[index].AsSpan().SequenceEqual(reopened.Imports[index].Header))
                throw new InvalidDataException("An import changed during serialization.");
        for (int index = 0; index < package.ExportCount; index++)
        {
            var before = exports[index];
            var after = reopened.Exports[index];
            if (before.InstancedFullPath != after.InstancedFullPath || before.ClassName != after.ClassName
                || !before.data.AsSpan().SequenceEqual(SHA256.HashData(after.Data)))
                throw new InvalidDataException("An export changed during serialization.");
        }
        serialized.Position = 0;
        string sha256 = Convert.ToHexStringLower(SHA256.HashData(serialized));
        serialized.Position = 0;
        using var destination = new FileStream(output, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        serialized.CopyTo(destination);
        destination.Flush(flushToDisk: true);
        Console.WriteLine(JsonSerializer.Serialize(new
        {
            protocol = 1, type = "validated", game = package.Game.ToString(),
            exports = package.ExportCount, size = destination.Length, sha256,
        }));
    }
}
