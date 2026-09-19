using System;
using System.IO;

namespace Deployd.Mele;

internal static class TextureCompileTests
{
    internal static void Run(string root)
    {
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

    private static void Reject(Action action)
    {
        try { action(); }
        catch (InvalidDataException) { return; }
        throw new InvalidOperationException("Invalid M3TO helper input was accepted.");
    }
}
