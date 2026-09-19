// Derived from ME3TweaksCore commit b5f006add38dbaea91dde3f16f45ecbc3b0155a9.
// Copyright ME3Tweaks contributors; GPL-3.0-only. Adapted with bounded validation.
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;
using LegendaryExplorerCore.Unreal.ObjectInfo;
using System;
using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using System.Linq;

namespace Deployd.Mele
{
    internal sealed class TextureOverrideTextureEntry
    {
        /// <summary>
        /// Minimum area of a texture before we oodle compress it in the package
        /// </summary>
        internal const int BTP_COMPRESS_SIZE_MIN = 64 * 64;

        // SHOULD ONLY CONTAIN TEXTURES!!
        // Examples: TO_BlueOutfit.pcc
        //           TO_A_BaseComponent.pcc
        //           TO_A_Armour_HeavyTextures.pcc
        /// <summary>
        /// Name of package to find this texture in, in the current folder. Can be relative.
        /// </summary>
        internal string CompilingSourcePackage { get; set; }

        /// <summary>
        /// Instanced full path of the texture in the source package
        /// </summary>
        internal string TextureIFP { get; set; }

        // SERIALIZATION =========================================================

        /// <summary>
        /// Serializes this texture to the BTP and BTPStream.
        /// </summary>
        /// <param name="compiler">The compiler that holds stats and other transient compile-time info</param>
        /// <param name="btpEntry">The BTP entry for this texture that we will be populating data into</param>
        /// <param name="btpStream">The stream we are serializing mip data to</param>
        /// <param name="metadataPackage">Optional package for storing the texture metadata when shipping a btp only.</param>
        internal void Serialize(TextureOverrideCompiler compiler, BTPTextureEntry btpEntry, Stream btpStream, ILazyLoadPackage package, IMEPackage metadataPackage)
        {
            // Find texture package
            //var packagePath = Path.Combine(sourceFolder, CompilingSourcePackage);
            //if (!File.Exists(packagePath))
            //{
            //    throw new Exception($"sourcepackage does not exist at location {packagePath}");
            //}

            // Load package and find texture
            // using var package = MEPackageHandler.UnsafePartialLoad(packagePath, x => x.InstancedFullPath.CaseInsensitiveEquals(TextureIFP));
            var texture = package.FindExport(TextureIFP);
            if (texture == null)
            {
                throw new InvalidDataException($"M3TO texture export '{TextureIFP}' was not found in its declared package.");
            }

            // Make sure it's Texture2D
            if (!texture.IsA(@"Texture2D"))
            {
                throw new InvalidDataException($"M3TO export '{TextureIFP}' is not a Texture2D.");
            }

            // Read metadata about texture.
            // We use just .From() so we can access it as LightMapTexture2D later.
            UTexture2D texBin = ObjectBinary.From(texture) as UTexture2D
                ?? throw new InvalidDataException("An M3TO Texture2D export has invalid binary data.");
            if (texBin.Mips.Count is < 1 or > 13)
                throw new InvalidDataException("An M3TO texture has an unsupported mip count.");
            var numPopulatedMips = texBin.Mips.Count(x => x.StorageType != StorageTypes.empty);
            var numEmptyMips = texBin.Mips.Count(x => x.StorageType == StorageTypes.empty);

            var props = texture.GetProperties();
            var tfc = props.GetProp<NameProperty>(@"TextureFileCacheName");
            var tfcGuidProp = props.GetProp<StructProperty>(@"TFCFileGuid");
            var format = props.GetProp<EnumProperty>(@"Format"); // Default would be PF_Unknown according to enum
            var lodBias = props.GetProp<IntProperty>(@"InternalFormatLODBias")?.Value ?? 0;
            var neverStream = props.GetProp<BoolProperty>(@"NeverStream")?.Value ?? false;
            var srgb = props.GetProp<BoolProperty>(@"sRGB")?.Value ?? true; // Default is true on Texture class

            // Set values on the btpEntry
            btpEntry.OverridePath = TextureIFP;
            btpEntry.PopulatedMipCount = (byte)numPopulatedMips;
            btpEntry.InternalFormatLODBias = lodBias;
            btpEntry.NeverStream = neverStream;
            btpEntry.bSRGB = srgb;
            // Format should always be set, in game defaults to Unknown if not set
            // this is caught in serialization in debug mode.
            if (format is not null && Enum.TryParse<BTPPixelFormat>(format.Value.Instanced, out var fmt))
            {
                btpEntry.Format = fmt;
            }
            if (btpEntry.Format == BTPPixelFormat.PF_Unknown || numPopulatedMips is < 1 or > 13)
                throw new InvalidDataException("An M3TO texture has unsupported format or mip metadata.");

            // Default
            btpEntry.TFC = btpEntry.Owner.TFCTable.GetTFC(null);
            if (tfc != null && tfc.Value.Name != null && tfcGuidProp != null)
            {
                // Fetch table index, add if not there
                btpEntry.TFC = btpEntry.Owner.TFCTable.GetOrAddTFC(tfc.Value.Name, CommonStructs.GetGuid(tfcGuidProp), texture);
            }

            // WRITING TEXTURE MIPS TO BTP STREAM =========================================
            compiler.serializedMipInfo.TryGetValue(TextureIFP, out var smi);

            // Write out populated mips data
            // The struct size is 13 mips. We go from largest mip to smallest mip
            // We could up from the smallest mip to the biggest
            for (int mipIndex = 0; mipIndex < texBin.Mips.Count; mipIndex++)
            {
                if (texBin.Mips.Count == mipIndex)
                    break;

                var sourceMip = texBin.Mips[mipIndex];
                var btpMip = btpEntry.Mips[mipIndex];

                // See if we already preprocessed this mip
                // in the texture compression step
                SerializedBTPMip serializedInfo = null;
                if (smi != null)
                {
                    smi.TryGetValue(mipIndex, out serializedInfo);
                }

                // Uncompressed size doesn't change from source mip
                btpMip.UncompressedSize = sourceMip.UncompressedSize;

                if (sourceMip.IsLocallyStored && !sourceMip.IsEmpty)
                {
                    // Will be prepared already so we must set matching size.
                    if (!sourceMip.IsCompressed)
                    {
                        // BTP mip will set compressed and uncompresed size equal,
                        // this will change if texture was oodle compressed
                        btpMip.CompressedSize = sourceMip.UncompressedSize;
                    }

                    // Are we a dedup mip?
                    bool isDedupMip = false;
                    if (serializedInfo == null)
                    {
                        serializedInfo = new SerializedBTPMip(sourceMip
#if DEBUG
                            ,
                            package,
                            texture,
                            mipIndex
#endif
                            );

                    }
                    else
                    {

                        if (serializedInfo.CompressedSize != sourceMip.Mip.Length)
                        {
                            serializedInfo = new SerializedBTPMip(sourceMip
#if DEBUG
                            ,
                            package,
                            texture,
                            mipIndex
#endif
                            );
                        }
                    }

                    if (serializedInfo.Offset == 0)
                    {
                        // We haven't serialized to BTP yet
                        if (serializedInfo.Digest is null)
                        {
                            serializedInfo.Digest = Convert.ToHexString(SHA256.HashData(sourceMip.Mip));
                            if (compiler.DedupMap.TryGetValue(serializedInfo.Digest, out var existing))
                            {
                                isDedupMip = true;
                                serializedInfo = existing;
                            }
                        }

                        if (!isDedupMip)
                        {
                            // we're going to write to end of the btp stream
                            serializedInfo.Offset = (ulong)btpStream.Length;
                        }
                    }
                    else
                    {
                        // offset was previously set; we're already serialized into btp
                        isDedupMip = true;
                    }

                    // record crc for future dedupe, but it must be 4x4 or larger since tiny textures have crc collisions
                    if (sourceMip.SizeX > 4 || sourceMip.SizeY > 4)
                    {
                        compiler.DedupMap[serializedInfo.Digest] = serializedInfo;
                    }

                    btpMip.CompressedSize = serializedInfo.CompressedSize;

                    // Serialize the mip data (if unique) and update the entry.
                    var data = isDedupMip ? null : sourceMip.Mip;
                    // Debug.WriteLine($@"Serializing {btpEntry.OverridePath} mip {mipIndex} so: {offset}, cs: {compressedSize}, data: {data?.Length}");
                    btpMip.SerializeData(btpStream,
                        (long)serializedInfo.Offset, // Dedup offset or end of stream
                        serializedInfo.CompressedSize, // Dedup compressed size or our mip's size
                        data // Only pass mip data if not a dedupe
                    );
                }
                else
                {
                    // It's a TFC offset
                    // Write 64bit version, it gets downcast later
                    btpMip.CompressedOffset = sourceMip.DataOffset;
                }

                btpMip.CompressedSize = serializedInfo?.CompressedSize ?? sourceMip.CompressedSize;
                btpMip.Width = (short)sourceMip.SizeX;
                btpMip.Height = (short)sourceMip.SizeY;
                btpMip.Flags = 0;
                if (!sourceMip.IsLocallyStored)
                {
                    // Set mip flag as stored in TFC
                    btpMip.Flags |= BTPMipFlags.External;
                }
                if (serializedInfo?.OodleCompressed == true)
                {
                    // Custom oodle compressed flag
                    // for the ASI.
                    btpMip.Flags |= BTPMipFlags.OodleCompressed;
                }
            }

            // Ensure parents are loaded for metadata export
            package.LoadExport(texture, true);

            // Write out metadata - texture export, then truncate it;


            var rop = new RelinkerOptionsPackage() { CheckImportsWhenExportingToPackage = false };
            EntryExporter.ExportExportToPackage(texture, metadataPackage, out var ported, customROP: rop);
            if (texBin is LightMapTexture2D lm2d)
            {
                // We need to keep track of the lightmap flags.
                // We simply store the flag value as the binary and will restore it later.
                (ported as ExportEntry).WriteBinary(BitConverter.GetBytes((int)lm2d.LightMapFlags));
            }
            else
            {
                // Empty binary in metadata
                (ported as ExportEntry).WriteBinary([]);
            }

            // Unload the export to reduce memory usage
            package.UnloadExport(texture);
        }
    }
}
