using System;
using System.IO;
using System.Linq;
using System.Threading.Tasks;

using LegendaryExplorerCore.Compression;

internal static class CodecTests
{
    private static int Main(string[] args)
    {
        string root = Path.Combine(Path.GetTempPath(), "deployd-codec-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        try
        {
            RejectsUnknownCodecIdentities();
            RejectsImplicitSearchAndArchiveCaches(root);
            RejectsLinksAndModifiedGameCodecs(root);
            if (args.Length == 1)
                RoundTripsVerifiedCodecAcrossThreads(args[0]);
            else if (args.Length != 0)
                throw new ArgumentException("Unexpected codec test arguments.");
            Console.WriteLine("Managed codec identity and boundary tests passed.");
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine("Managed codec test failed: " + error.GetType().Name);
            return 1;
        }
        finally
        {
            Directory.Delete(root, recursive: true);
        }
    }

    private static void RejectsUnknownCodecIdentities()
    {
        Reject<InvalidDataException>(() => OodleHelper.VerifyIdentity(Array.Empty<byte>()));
        Reject<InvalidDataException>(() => OodleHelper.VerifyIdentity(new byte[OodleHelper.CodecSize - 1]));
        Reject<InvalidDataException>(() => OodleHelper.VerifyIdentity(new byte[OodleHelper.CodecSize + 1]));
        byte[] modified = new byte[OodleHelper.CodecSize];
        modified[0] = (byte)'M';
        modified[1] = (byte)'Z';
        Reject<InvalidDataException>(() => OodleHelper.VerifyIdentity(modified));
    }

    private static void RejectsImplicitSearchAndArchiveCaches(string root)
    {
        Require(!OodleHelper.EnsureOodleDll(), "Implicit game lookup was accepted.");
        Require(!OodleHelper.EnsureOodleDll(root, root), "An archive cache was accepted.");
    }

    private static void RejectsLinksAndModifiedGameCodecs(string root)
    {
        string game = Path.Combine(root, "game");
        string binary = Path.Combine(game, "Binaries", "Win64", "oo2core_8_win64.dll");
        Directory.CreateDirectory(Path.GetDirectoryName(binary));
        File.WriteAllBytes(binary, new byte[OodleHelper.CodecSize]);
        Reject<InvalidDataException>(() => OodleHelper.EnsureOodleDll(game));
        File.Delete(binary);
        string target = Path.Combine(root, "external.dll");
        File.WriteAllBytes(target, new byte[OodleHelper.CodecSize]);
        File.CreateSymbolicLink(binary, target);
        Reject<InvalidDataException>(() => OodleHelper.EnsureOodleDll(game));
        File.Delete(binary);
        string link = Path.Combine(root, "linked-game");
        Directory.CreateSymbolicLink(link, game);
        Reject<InvalidDataException>(() => OodleHelper.EnsureOodleDll(link));
        Directory.Delete(link);
        link = Path.Combine(root, "linked-parent");
        Directory.CreateSymbolicLink(link, root);
        Reject<InvalidDataException>(() => OodleHelper.EnsureOodleDll(Path.Combine(link, "game")));
        Directory.Delete(link);
    }

    private static void RoundTripsVerifiedCodecAcrossThreads(string game)
    {
        string codec = Path.Combine(game, "Binaries", "Win64", "oo2core_8_win64.dll");
        byte[] original = File.ReadAllBytes(codec);
        byte[] modified = (byte[])original.Clone();
        modified[modified.Length / 2] ^= 1;
        Reject<InvalidDataException>(() => OodleHelper.VerifyIdentity(modified));
        Require(OodleHelper.EnsureOodleDll(game), "The verified codec was not loaded.");
        Parallel.For(0, 16, index =>
        {
            byte[] input = new byte[262144 + index];
            new Random(index).NextBytes(input);
            byte[] compressed = OodleHelper.Compress(input);
            byte[] output = new byte[input.Length];
            Require(OodleHelper.Decompress(compressed, output) == input.Length && input.SequenceEqual(output),
                "A managed codec round trip changed its input.");
        });
        Reject<InvalidDataException>(() => OodleHelper.GetCompressionBound(0));
        Reject<InvalidDataException>(() => OodleHelper.Compress(new byte[16], new byte[1]));
        Reject<InvalidDataException>(() => OodleHelper.Decompress(new byte[16], new byte[16]));
        Require(original.SequenceEqual(File.ReadAllBytes(codec)), "The game codec was modified.");
    }

    private static void Reject<T>(Action operation) where T : Exception
    {
        try { operation(); }
        catch (T) { return; }
        throw new InvalidOperationException("An invalid codec operation was accepted.");
    }

    private static void Require(bool condition, string message)
    {
        if (!condition)
            throw new InvalidOperationException(message);
    }
}
