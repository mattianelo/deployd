using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;

namespace LegendaryExplorerCore.Compression;

public static class OodleHelper
{
    internal const int CodecSize = 1007616;
    internal const string CodecSha256 = "d42940381611cda3b8555f6eb9fcb1bc3b1a3b96d7e24cb98738f4b71653d415";
    private const string Adapter = "deployd_oodle";
    private static readonly object Sync = new();
    private static bool loaded;

    static OodleHelper()
    {
        NativeLibrary.SetDllImportResolver(typeof(OodleHelper).Assembly, (name, assembly, path) =>
            name == Adapter
                ? NativeLibrary.Load(Path.Combine(AppContext.BaseDirectory, "libdeployd_oodle.so"))
                : IntPtr.Zero);
    }

    [DllImport(Adapter, EntryPoint = "mele_oodle_load_verified", CallingConvention = CallingConvention.Cdecl)]
    private static extern unsafe int LoadVerified(byte* data, ulong size);

    [DllImport(Adapter, EntryPoint = "mele_oodle_bound", CallingConvention = CallingConvention.Cdecl)]
    private static extern long Bound(ulong size);

    [DllImport(Adapter, EntryPoint = "mele_oodle_compress", CallingConvention = CallingConvention.Cdecl)]
    private static extern unsafe long Encode(byte* input, ulong size, byte* output, ulong capacity);

    [DllImport(Adapter, EntryPoint = "mele_oodle_decompress", CallingConvention = CallingConvention.Cdecl)]
    private static extern unsafe long Decode(byte* input, ulong size, byte* output, ulong capacity);

    // LEC's implicit search must never find a DLL in an archive, cache, or working directory.
    public static bool EnsureOodleDll(string gameRootPath = null, string storagePath = null)
    {
        lock (Sync)
        {
            if (loaded)
                return true;
            if (gameRootPath is null || storagePath is not null)
                return false;
            string root = Path.GetFullPath(gameRootPath);
            for (DirectoryInfo ancestor = new(root); ancestor is not null; ancestor = ancestor.Parent)
                if ((ancestor.Attributes & FileAttributes.ReparsePoint) != 0)
                    throw new InvalidDataException("The game codec root contains a symbolic link.");
            string path = root;
            foreach (string component in new[] { "", "Binaries", "Win64", "oo2core_8_win64.dll" })
            {
                if (component.Length != 0)
                    path = Path.Combine(path, component);
                if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
                    throw new InvalidDataException("The game codec location contains a symbolic link.");
            }
            using var input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
            if (input.Length != CodecSize)
                throw new InvalidDataException("The game codec size does not match the approved version.");
            byte[] bytes = new byte[CodecSize];
            input.ReadExactly(bytes);
            if (input.ReadByte() != -1)
                throw new InvalidDataException("The game codec changed while being verified.");
            VerifyIdentity(bytes);
            unsafe
            {
                fixed (byte* pointer = bytes)
                {
                    if (LoadVerified(pointer, (ulong)bytes.Length) != 0)
                        throw new InvalidOperationException("The native loader could not initialize the verified game codec.");
                }
            }
            loaded = true;
            return true;
        }
    }

    internal static void VerifyIdentity(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != CodecSize || !CryptographicOperations.FixedTimeEquals(
                SHA256.HashData(bytes), Convert.FromHexString(CodecSha256)))
            throw new InvalidDataException("The game codec does not match the approved SHA-256 identity.");
    }

    public static int GetCompressionBound(int size) => CheckedResult(Bound(checked((ulong)size)));

    public static unsafe int Compress(ReadOnlySpan<byte> input, Span<byte> output)
    {
        fixed (byte* source = input)
        fixed (byte* destination = output)
            return CheckedResult(Encode(source, (ulong)input.Length, destination, (ulong)output.Length));
    }

    public static byte[] Compress(ReadOnlySpan<byte> input)
    {
        byte[] output = new byte[GetCompressionBound(input.Length)];
        int size = Compress(input, output);
        return output.AsSpan(0, size).ToArray();
    }

    public static unsafe int Decompress(ReadOnlySpan<byte> input, Span<byte> output)
    {
        fixed (byte* source = input)
        fixed (byte* destination = output)
            return CheckedResult(Decode(source, (ulong)input.Length, destination, (ulong)output.Length));
    }

    private static int CheckedResult(long result) => result > 0 && result <= int.MaxValue
        ? (int)result
        : throw new InvalidDataException("The game codec rejected a compressed block or buffer size.");
}
