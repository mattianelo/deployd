using System;
using System.IO;
using System.Text;
using System.Text.Json;

using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;

namespace Deployd.Mele;

internal static class TextureCompileTests
{
    internal static void Run(string root)
    {
        AcceptsUpstreamManifestSyntax();
        DistinguishesExternalAndInlineMips();
        string game = Path.Combine(root, "texture-game");
        string input = Path.Combine(root, "texture-input");
        string output = Path.Combine(root, "texture-output");
        foreach (string directory in new[] { game, input, output }) Directory.CreateDirectory(directory);
        var manifest = new InputFile("DLC/DLC_MOD_Test/CookedPCConsole/TextureOverride-Test.m3to", 1, new string('a', 64));
        var package = new InputFile("DLC/DLC_MOD_Test/CookedPCConsole/TO_Test.pcc", 1, new string('b', 64));
        string[] outputs = { "DLC/DLC_MOD_Test/CombinedTextureOverrides.btp", "DLC/DLC_MOD_Test/BTPMetadata.btm" };
        var request = new TextureRequest(1, "mele-m3to", game, input, output, "LE2", "DLC_MOD_Test",
            new[] { manifest }, new[] { package }, outputs, 1);
        TextureCompile.Validate(request);
        Reject(() => TextureCompile.Validate(request with { Protocol = 2 }));
        Reject(() => TextureCompile.Validate(request with { Operation = "future" }));
        Reject(() => TextureCompile.Validate(request with { Game = "ME2" }));
        Reject(() => TextureCompile.Validate(request with { Dlc = "DLC_EXP_Test" }));
        Reject(() => TextureCompile.Validate(request with { OutputRoot = input }));
        Reject(() => TextureCompile.Validate(request with { Textures = 0 }));
        Reject(() => TextureCompile.Validate(request with { Packages = new[] { package with { Size = 512L * 1024 * 1024 + 1 } } }));
        Reject(() => TextureCompile.Validate(request with { Manifests = new[] { manifest, manifest } }));
        Reject(() => TextureCompile.Validate(request with { Outputs = new[] { outputs[1], outputs[0] } }));
    }

    private static void DistinguishesExternalAndInlineMips()
    {
        var mip = new UTexture2D.Texture2DMipMap
        {
            SizeX = 64, SizeY = 64, UncompressedSize = 4096,
            CompressedSize = 1024, StorageType = StorageTypes.extOodle,
        };
        if (TextureOverrideCompiler.HasInlineData(mip))
            throw new InvalidOperationException("An external mip was treated as inline data.");
        mip.StorageType = StorageTypes.empty;
        if (TextureOverrideCompiler.HasInlineData(mip))
            throw new InvalidOperationException("An empty mip was treated as inline data.");
        mip.StorageType = StorageTypes.pccUnc;
        Reject(() => TextureOverrideCompiler.HasInlineData(mip));
        mip.Mip = new byte[4096];
        if (!TextureOverrideCompiler.HasInlineData(mip))
            throw new InvalidOperationException("An inline mip was skipped.");
        mip.StorageType = StorageTypes.extOodle;
        mip.SizeX = 4097;
        Reject(() => TextureOverrideCompiler.HasInlineData(mip));
        mip.SizeX = 64;
        mip.CompressedSize = -1;
        Reject(() => TextureOverrideCompiler.HasInlineData(mip));
    }

    private static void AcceptsUpstreamManifestSyntax()
    {
        const string source = "\uFEFF{/* comment */\"Game\":\"LE2\",\"comment\":\"https://example.org/,}\",\"Textures\":[{\"SourcePackage\":\"TO_Test.pcc\",\"TextureIFP\":\"Example.Texture\",\"note\":true,},],// end\n}";
        using var stream = new MemoryStream(Encoding.UTF8.GetBytes(source));
        var manifest = TextureCompile.ReadManifest(stream);
        if (manifest.Game != "LE2" || manifest.Textures.Length != 1
            || manifest.Textures[0].Sourcepackage != "TO_Test.pcc"
            || manifest.Textures[0].Textureifp != "Example.Texture")
            throw new InvalidOperationException("Upstream M3TO syntax changed the texture declaration.");
        foreach (string invalid in new[]
        {
            "{\"game\":\"LE2\",\"Game\":\"LE1\",\"textures\":[]}",
            "{\"game\":\"LE2\",\"textures\":[,]}",
            "{\"game\":\"LE2\",\"textures\":true}",
            "{\"game\":\"LE2\",\"textures\":[],} trailing",
            "{/* unterminated",
        })
        {
            using var bad = new MemoryStream(Encoding.UTF8.GetBytes(invalid));
            try { TextureCompile.ReadManifest(bad); }
            catch (Exception error) when (error is JsonException or InvalidDataException) { continue; }
            throw new InvalidOperationException("Invalid M3TO syntax was accepted.");
        }
    }

    private static void Reject(Action action)
    {
        try { action(); }
        catch (InvalidDataException) { return; }
        throw new InvalidOperationException("Invalid M3TO helper input was accepted.");
    }
}
