// Derived from ME3TweaksCore commit b5f006add38dbaea91dde3f16f45ecbc3b0155a9.
// Copyright ME3Tweaks contributors; GPL-3.0-only. Adapted for Deployd's isolated helper.
using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Helpers;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;
using LegendaryExplorerCore.Unreal.ObjectInfo;
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.IO.Hashing;
using System.Linq;
using System.Security.Cryptography;
using System.Threading;
using System.Threading.Tasks;

namespace Deployd.Mele
{
    internal sealed class TextureOverrideCompiler
    {
        internal string DLCName;

        // DEDUPLICATION ========================
        /// <summary>
        /// Maps the CRC of a mip to its offset in the CRC map.
        /// </summary>
        internal Dictionary<string, SerializedBTPMip> DedupMap = new(StringComparer.Ordinal);


        // PROGRESS ============================
        /// <summary>
        /// Current progress info object. Can be null
        /// </summary>

        // Statistics ===========================
        // SERIALIZATION ONLY
        // Total amount of all uncompressed data added to btp
        internal long InDataSize = 0;
        // Size of BTP data segment
        internal long OutDataSize = 0;
        // Amount of data that was deduplicated in BTP
        internal long DeduplicationSavings = 0;


        /// <summary>
        /// Contains information about IFP -> serialized information. Maps IFP to dictionary of mip indices that were serialized to the BTP data block.
        /// </summary>
        internal ConcurrentDictionary<string, Dictionary<int, SerializedBTPMip>> serializedMipInfo = new();

        /// <summary>
        /// Converts a texture override manifest and its supporting data into a Binary Texture Package
        /// </summary>
        /// <param name="tom">Manifest to convert</param>
        /// <param name="sourceFolder">Source folder that contains the packages. This is the DLC cooked directory.</param>
        /// <param name="btpStream">The destination stream to write to. This should be the start of a stream.</param>
        /// <param name="pi">Progress interop</param>
        internal void Build(TextureManifest manifest, string sourceFolder, Stream btpStream, string dlcName, Action<int, int> progress, IMEPackage metadataPackage)
        {

            // Setup variables!
            DLCName = dlcName;

            // DEBUG ONLY
            // Filter for testing
            // manifest.Textures = manifest.Textures.Where(x => x.TextureIFP.Contains(@"BIOG_HMM_HED_PROMorph.Eye.EYE_Diff", StringComparison.OrdinalIgnoreCase)).ToList(); // == "BIOG_Humanoid_MASTER_MTR_R.Eye.HMM_EYE_MASTER_Diffuse").ToList();

            // Prepare for performance by serializing in-order of source package
            manifest.Textures = manifest.Textures
                .OrderBy(texture => texture.CompilingSourcePackage, StringComparer.OrdinalIgnoreCase)
                .ThenBy(texture => texture.TextureIFP, StringComparer.OrdinalIgnoreCase)
                .ToList();


            // Start serialization
            // We use BTP object only for transient data storage,
            // it does not handle actual serialization as we would have to
            // load mips into it, which could use huge amounts of memory.
            // This is effectively a lazy serializer
            var BTP = new BinaryTexturePackage();

            // Add No-TFC to TFC table at index 0
            BTP.TFCTable.GetTFCTableIndex(@"None", Guid.Empty, null);

            // Setup header (first pass)
            var fnvInput = $@"{manifest.Game}{DLCName}";
            BTP.Header.TargetHash = FNV1.Compute(fnvInput);
            BTP.Header.Serialize(btpStream);

            // TEXTURE OVERRIDE SERIALIZATION =======================
            var total = manifest.Textures.Count;
            var done = 0;


            // Preallocate space for texture entries

            BTP.TextureOverrides = new(total);
            for (var i = 0; i < total; i++)
            {
                BTP.TextureOverrides.Add(new BTPTextureEntry(BTP, null));
            }

            foreach (var to in BTP.TextureOverrides)
            {
                // Write out blank placeholders for now so the data is allocated in the stream
                done++;
                to.Serialize(btpStream);
                BTP.Header.TextureCount++;
            }

            // Where data for mips begins being added
            // Serialize texture entries and mip data
            ILazyLoadPackage currentSourcePackage = null;
            done = 0;

            for (done = 0; done < BTP.TextureOverrides.Count; done++)
            {
                var btpEntry = BTP.TextureOverrides[done];
                var texture = manifest.Textures[done];

                if (currentSourcePackage == null || !currentSourcePackage.FilePath.EndsWith(texture.CompilingSourcePackage, StringComparison.OrdinalIgnoreCase))
                {
                    // Load new source package
                    var newPath = Path.Combine(sourceFolder, texture.CompilingSourcePackage);
                    if (!File.Exists(newPath))
                    {
                        throw new InvalidDataException($"Referenced M3TO source package '{texture.CompilingSourcePackage}' is missing.");
                    }

                    currentSourcePackage?.Dispose(); // Dispose any existing package to lose the stream
                    currentSourcePackage = MEPackageHandler.UnsafeLazyLoad(newPath);
                    if (currentSourcePackage.Game != manifest.Game)
                        throw new InvalidDataException("An M3TO source package belongs to another game.");

                    // Dump old package.
                    GC.Collect();

                    // Now compress textures in the package in parallel to speed things along.
                    PrepareTextureCompression(currentSourcePackage, manifest.Textures
                        .Where(item => item.CompilingSourcePackage.Equals(texture.CompilingSourcePackage, StringComparison.OrdinalIgnoreCase))
                        .Select(item => item.TextureIFP));
                }

                // Serialize textures
                texture.Serialize(this, btpEntry, btpStream, currentSourcePackage, metadataPackage);
                if (btpStream.Length > 4L * 1024 * 1024 * 1024)
                    throw new InvalidDataException("M3TO compiled output exceeds 4 GiB.");
                int completed = done + 1;
                int interval = Math.Max(1, (total + 4095) / 4096);
                if (completed == total || completed % interval == 0)
                    progress(completed, total);

            }

            // Lose the reference
            currentSourcePackage?.Dispose();
            currentSourcePackage = null;


            metadataPackage.Save();

            // Compute metadata crc
            var metadataInfo = new FileInfo(metadataPackage.FilePath);
            if (metadataInfo.Length is < 1 or > 16 * 1024 * 1024)
                throw new InvalidDataException("M3TO metadata output exceeds 16 MiB.");
            var metadataBytes = File.ReadAllBytes(metadataPackage.FilePath);
            BTP.Header.MetadataCRC = Crc32.HashToUInt32(metadataBytes);

            // Log statistics
            BTP.FinalSerialize(btpStream);

            // Done
            btpStream.Flush();
        }

        /// <summary>
        /// Returns if an export is something stored in a BTP
        /// </summary>
        /// <param name="export">Export to check</param>
        /// <returns></returns>
        private bool IsBTPTexture(ExportEntry export)
        {
            return export.IsA(@"Texture2D");
        }

        /// <summary>
        /// Sets information about how to serialize mip data
        /// </summary>
        /// <param name="instancedFullPath"></param>
        /// <param name="i">Mip index from the source export</param>
        /// <param name="serializationInfo"></param>
        private void SetSerializationInfo(string instancedFullPath, int i, SerializedBTPMip serializationInfo)
        {
            if (!serializedMipInfo.TryGetValue(instancedFullPath, out var existingMap))
            {
                existingMap = new(6);
                serializedMipInfo[instancedFullPath] = existingMap;
            }

            existingMap[i] = serializationInfo;
        }


        /// <summary>
        /// This method enumerates textures in the target package and compresses their mips with Oodle compression in parallel for performance
        /// </summary>
        /// <param name="currentSourcePackage"></param>
        private void PrepareTextureCompression(ILazyLoadPackage currentSourcePackage, IEnumerable<string> texturePaths)
        {
            var requested = texturePaths.ToHashSet(StringComparer.OrdinalIgnoreCase);
            var loadedItems = currentSourcePackage.Exports.Where(export => requested.Contains(export.InstancedFullPath));
            if (loadedItems.Count() != requested.Count)
                throw new InvalidDataException("An M3TO texture export is missing or ambiguous.");
            foreach (var texture in loadedItems)
            {
                if (!IsBTPTexture(texture))
                    throw new InvalidDataException("An M3TO override does not identify a Texture2D export.");
                var compressedAny = false;
                if (!texture.IsDataLoaded())
                {
                    currentSourcePackage.LoadExport(texture);
                }

                // We must use .From without typing so we get a full object back for lightmaps.
                var texBin = ObjectBinary.From(texture) as UTexture2D
                    ?? throw new InvalidDataException("An M3TO Texture2D export has invalid binary data.");
                if (texBin.Mips.Count is < 1 or > 13)
                    throw new InvalidDataException("An M3TO texture has an unsupported mip count.");
                for (int i = 0; i < texBin.Mips.Count; i++)
                {
                    var sourceMip = texBin.Mips[i];
                    if (sourceMip.SizeX is < 1 or > 4096 || sourceMip.SizeY is < 1 or > 4096
                        || sourceMip.Mip.Length > 256 * 1024 * 1024 || sourceMip.UncompressedSize is < 0 or > 256 * 1024 * 1024
                        || sourceMip.CompressedSize is < 0 or > 256 * 1024 * 1024)
                        throw new InvalidDataException("An M3TO texture mip exceeds its safety limits.");
                    if (!sourceMip.IsLocallyStored || sourceMip.StorageType == StorageTypes.empty)
                        continue; // Nothing to see here

                    var serializationInfo = new SerializedBTPMip();
                    serializationInfo.Digest = Convert.ToHexString(SHA256.HashData(sourceMip.Mip));
                    serializationInfo.CompressedSize = sourceMip.CompressedSize; // Copy original value

                    // Compress textures that would be big for space savings
                    // texture must have >= 64x64 size and not already compressed
                    if (!sourceMip.IsCompressed)
                    {
                        InDataSize = checked(InDataSize + sourceMip.Mip.Length);
                        var area = checked(sourceMip.SizeX * sourceMip.SizeY);
                        if (area >= TextureOverrideTextureEntry.BTP_COMPRESS_SIZE_MIN)
                        {
                            sourceMip.StorageType = StorageTypes.pccOodle;
                            sourceMip.Mip = OodleHelper.Compress(sourceMip.Mip); // compress mip and store it back
                            serializationInfo.CompressedSize = sourceMip.CompressedSize = sourceMip.Mip.Length; // Now set the new compressed size so we can use it later
                            serializationInfo.OodleCompressed = true; // Flag that makes it think its compressed.
#if DEBUG
                            serializationInfo.DebugSource = $@"{currentSourcePackage.FilePath} {texture.InstancedFullPath} mip {i}";
#endif
                            compressedAny = true;
                        }
                    }
                    OutDataSize = checked(OutDataSize + sourceMip.Mip.Length);
                    if (OutDataSize > 4L * 1024 * 1024 * 1024)
                        throw new InvalidDataException("M3TO compiled output exceeds 4 GiB.");
                    SetSerializationInfo(texture.InstancedFullPath, i, serializationInfo);

                }

                if (compressedAny)
                {
                    texture.WriteBinary(texBin);
                }
            }
        }
    }
}
