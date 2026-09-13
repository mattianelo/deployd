using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Threading;

using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal.BinaryConverters;

namespace Deployd.Mele;

internal static class ShaderMergeTests
{
    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidDataException(message);
    }

    private static void Reject(Action action)
    {
        try { action(); } catch (InvalidDataException) { return; } catch (EndOfStreamException) { return; }
        throw new InvalidDataException("Invalid shader input was accepted.");
    }

    internal static byte[] Bytecode(byte marker)
    {
        var bytes = new byte[48];
        using var writer = new BinaryWriter(new MemoryStream(bytes, writable: true));
        writer.Write("DXBC"u8);
        writer.BaseStream.Position = 20;
        writer.Write(1); writer.Write(48); writer.Write(1); writer.Write(36);
        writer.Write("SHDR"u8); writer.Write(4); writer.Write((int)marker);
        return bytes;
    }

    private static byte[] Cache(MEGame game)
    {
        using var stream = new MemoryStream();
        using var writer = new BinaryWriter(stream);
        void Name(string text) { writer.Write(text.Length + 1); writer.Write(System.Text.Encoding.Latin1.GetBytes(text)); writer.Write((byte)0); }
        writer.Write("BMSG"u8);
        writer.Write((int)UnrealPackageFile.UnrealVersion(game));
        writer.Write((int)UnrealPackageFile.LicenseeVersion(game));
        writer.Write((byte)5); writer.Write(0);
        writer.Write(1); Name("RetainedNameMapEntry");
        writer.Write(2);
        for (byte index = 0; index < 2; index++)
        {
            Name("FResolveVertexShader");
            var id = new byte[16]; id[0] = index;
            writer.Write(id);
            long pointer = stream.Position;
            writer.Write(0); writer.Write((byte)5); writer.Write((byte)0);
            var code = Bytecode(index);
            writer.Write(code.Length); writer.Write(code); writer.Write(99);
            writer.Write(id); Name("FResolveVertexShader"); writer.Write(10);
            long end = stream.Position;
            stream.Position = pointer; writer.Write((int)end); stream.Position = end;
        }
        writer.Write(0);
        return stream.ToArray();
    }

    internal static void Run(string root)
    {
        var bytes = Bytecode(1);
        ShaderMerge.ValidateBytecode(bytes);
        for (int length = 0; length < bytes.Length; length++) Reject(() => ShaderMerge.ValidateBytecode(bytes[..length]));
        foreach (var game in new[] { MEGame.LE1, MEGame.LE2, MEGame.LE3 })
        {
            var baseline = Cache(game);
            var cache = new GlobalShaderFile(baseline, game);
            Require(cache.Replace(new Dictionary<int, byte[]>(), CancellationToken.None).SequenceEqual(baseline), "Empty shader merge changed original bytes.");
            Reject(() => new GlobalShaderFile(baseline, game == MEGame.LE1 ? MEGame.LE2 : MEGame.LE1));
            var changed = cache.Replace(new Dictionary<int, byte[]> { [0] = Bytecode(9) }, CancellationToken.None);
            var reverted = new GlobalShaderFile(changed, game).Replace(new Dictionary<int, byte[]> { [0] = Bytecode(0) }, CancellationToken.None);
            Require(reverted.SequenceEqual(baseline), "Shader rewrite changed unrelated bytes or the name map.");
            Reject(() => cache.Replace(new Dictionary<int, byte[]> { [2] = Bytecode(2) }, CancellationToken.None));
            for (int length = 0; length < baseline.Length; length++) Reject(() => new GlobalShaderFile(baseline[..length], game));
            Lifecycle(Path.Combine(root, "shader-" + game), baseline, game, Bytecode(8), Bytecode(9));
        }
    }

    private static InputFile Write(string root, string relative, byte[] bytes)
    {
        string path = Path.Combine(root, relative);
        Directory.CreateDirectory(Path.GetDirectoryName(path));
        File.WriteAllBytes(path, bytes);
        return new(relative, bytes.Length, Convert.ToHexStringLower(SHA256.HashData(bytes)));
    }

    private static void Lifecycle(string root, byte[] baseline, MEGame game, byte[] low, byte[] high)
    {
        foreach (string name in new[] { "game", "original", "input", "output" }) Directory.CreateDirectory(Path.Combine(root, name));
        string original = Path.Combine(root, "original"), input = Path.Combine(root, "input");
        var target = new TargetPackage(Write(original, ShaderMerge.Target, baseline), Write(input, ShaderMerge.Target, baseline));
        var a = new ShaderContribution("DLC_MOD_A", 20, 0, Write(input, "DLC/DLC_MOD_A/CookedPCConsole/GlobalShader-0-high.m3gs", high));
        var b = new ShaderContribution("DLC_MOD_B", 5, 0, Write(input, "DLC/DLC_MOD_B/CookedPCConsole/GlobalShader-0-low.m3gs", low));
        var request = new ShaderRequest(1, "mele-m3gs", Path.Combine(root, "game"), original, input, Path.Combine(root, "output"), game.ToString(), target, new[] { a, b });
        byte[] Apply(string name, params ShaderContribution[] items)
        {
            string output = Path.Combine(root, name);
            Directory.CreateDirectory(output);
            var progress = new List<int>();
            var outputs = ShaderMerge.Execute(request with { OutputRoot = output, Contributions = items }, CancellationToken.None, (done, total) => { Require(total == items.Length, "Wrong shader progress total."); progress.Add(done); });
            Require(progress.SequenceEqual(Enumerable.Range(1, items.Length)), "Shader progress changed order.");
            var data = File.ReadAllBytes(Path.Combine(output, ShaderMerge.Target));
            Require(outputs.Length == 1 && outputs[0].Size == data.Length && outputs[0].Sha256 == Convert.ToHexStringLower(SHA256.HashData(data)), "Wrong shader output manifest.");
            return data;
        }
        byte[] merged = Apply("first", a, b);
        Require(new GlobalShaderFile(merged, game).Bytecode(0).SequenceEqual(high), "Higher DLC mount did not win.");
        Write(input, ShaderMerge.Target, merged);
        request = request with { Target = target with { Current = Write(input, ShaderMerge.Target, merged) } };
        Require(Apply("rebuild", b, a).SequenceEqual(merged), "Shader rebuild retained previous contributions.");
        Require(new GlobalShaderFile(Apply("disabled", b), game).Bytecode(0).SequenceEqual(low), "Disabling the higher DLC did not reveal the lower replacement.");
        Require(Apply("removed").SequenceEqual(baseline), "Removing shaders did not restore original bytes.");
        var duplicate = a with { Shader = Write(input, "DLC/DLC_MOD_A/CookedPCConsole/GlobalShader-00-other.m3gs", low) };
        Reject(() => ShaderMerge.Validate(request with { Contributions = new[] { a, duplicate } }));
        Reject(() => ShaderMerge.Validate(request with { Contributions = new[] { a, b with { Mount = a.Mount } } }));
        var invalid = a with { Index = 100000, Shader = Write(input, "DLC/DLC_MOD_A/CookedPCConsole/GlobalShader-100000-bad.m3gs", high) };
        Reject(() => ShaderMerge.Execute(request with { Contributions = new[] { invalid } }, CancellationToken.None, (_, _) => { }));
        Write(input, a.Shader.Path, low);
        Reject(() => ShaderMerge.Execute(request, CancellationToken.None, (_, _) => { }));
        Require(!Directory.EnumerateFileSystemEntries(request.OutputRoot).Any(), "Failed shader job published an output.");
        using var cancellation = new CancellationTokenSource();
        cancellation.Cancel();
        try { ShaderMerge.Execute(request, cancellation.Token, (_, _) => { }); throw new InvalidDataException("Shader cancellation was ignored."); }
        catch (OperationCanceledException) { }
        Require(File.ReadAllBytes(Path.Combine(original, ShaderMerge.Target)).SequenceEqual(baseline), "Shader job changed its baseline.");
    }

    internal static void Corpus(string root)
    {
        foreach (var game in new[] { MEGame.LE1, MEGame.LE2, MEGame.LE3 })
        {
            byte[] bytes = File.ReadAllBytes(Path.Combine(root, game + ".bin"));
            var cache = new GlobalShaderFile(bytes, game);
            using var source = new MemoryStream(bytes);
            var reference = GlobalShaderCache.ReadGlobalShaderCache(source, game);
            var shaders = reference.Shaders.Values.ToArray();
            Require(cache.Shaders.Length == shaders.Length, "Shader index ordering differs from pinned tooling.");
            for (int index = 0; index < shaders.Length; index++) Require(cache.Bytecode(index).SequenceEqual(shaders[index].ShaderByteCode), "Shader bytes differ from pinned tooling.");
            var candidates = shaders.Select(shader => shader.ShaderByteCode).Where(code => code.AsSpan().StartsWith("DXBC"u8)).ToArray();
            Require(candidates.Length > 1, "Game cache lacks reference DXBC shaders.");
            byte[] low = candidates[0], high = candidates.First(code => code.Length != low.Length);
            ShaderMerge.ValidateBytecode(low); ShaderMerge.ValidateBytecode(high);
            var changed = cache.Replace(new Dictionary<int, byte[]> { [0] = high }, CancellationToken.None);
            using var changedStream = new MemoryStream(changed);
            var reopened = GlobalShaderCache.ReadGlobalShaderCache(changedStream, game);
            shaders[0].ShaderByteCode = high;
            using var referenceBytes = new MemoryStream();
            var serializer = new PackagelessSerializingContainer(referenceBytes, null);
            serializer.SetGame(game);
            reference.WriteTo(serializer);
            using var actualBytes = new MemoryStream();
            var actualSerializer = new PackagelessSerializingContainer(actualBytes, null);
            actualSerializer.SetGame(game);
            reopened.WriteTo(actualSerializer);
            Require(referenceBytes.ToArray().SequenceEqual(actualBytes.ToArray()), "Global shader output differs semantically from pinned tooling.");
            Lifecycle(Path.Combine(root, game.ToString()), bytes, game, low, high);
            Console.WriteLine($"{game}: M3GS merge, reference comparison, rebuild, removal and failures passed ({shaders.Length} shaders).");
        }
    }
}
