// Derived from ME3TweaksCore commit b5f006add38dbaea91dde3f16f45ecbc3b0155a9.
// Copyright ME3Tweaks contributors; GPL-3.0-only. Adapted for bounded serialization only.
using LegendaryExplorerCore.Compression;
using LegendaryExplorerCore.Gammtek.Extensions.Collections.Generic;
using LegendaryExplorerCore.Gammtek.IO;
using LegendaryExplorerCore.Helpers;
using LegendaryExplorerCore.Misc;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;
using LegendaryExplorerCore.Unreal.Classes;
using LegendaryExplorerCore.Unreal.ObjectInfo;
using Newtonsoft.Json;
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Hashing;
using System.Linq;
using System.Text;

namespace Deployd.Mele
{
    /// <summary>
    /// Contains information about a mip stored in BTP. We only need the offset and how big the chunk of data is at that offset. This is used for deduplication.
    /// </summary>
    internal sealed class SerializedBTPMip
    {
        /// <summary>
        /// The offset in the BTP where the mip data resides. If zero, it is not set yet.
        /// </summary>
        public ulong Offset;

        /// <summary>
        /// The compressed size of the mip (either oodle or original)
        /// </summary>
        public int CompressedSize;

        public string Digest;

        public SerializedBTPMip()
        {

        }

        /// <summary>
        /// Contains transient information about a mip being serialized into a BTP.
        /// </summary>
        /// <param name="sourceMip"></param>
        /// <param name="package"></param>
        /// <param name="texture"></param>
        /// <param name="mipIndex"></param>
        public SerializedBTPMip(UTexture2D.Texture2DMipMap sourceMip
#if DEBUG
            ,
            IMEPackage package,
            ExportEntry texture,
            int mipIndex
#endif
            )
        {
            // Was not compressed, so just set the size the same.
            CompressedSize = sourceMip.Mip.Length;
#if DEBUG
            SizeX = sourceMip.SizeX;
            SizeY = sourceMip.SizeY;
            DebugSource = $@"{package.FilePath} {texture.InstancedFullPath} mip {mipIndex}";
#endif
        }

        /// <summary>
        /// If mip was oodle compressed
        /// </summary>
        public bool OodleCompressed { get; internal set; }

#if DEBUG
        // Used to determine if we can trust crc
        public int SizeY { get; internal set; }
        public int SizeX { get; internal set; }

        /// <summary>
        /// Where this serialization info was generated from
        /// </summary>
        public string DebugSource { get; internal set; }
#endif
    }

    /// <summary>
    /// Class for serializing FNV1 hashes. Very basic.
    /// </summary>
    internal sealed class FNV1
    {
        private const uint OFFSET = 0x811c9dc5;
        private const uint PRIME = 0x01000193;

        /// <summary>
        /// Computes the FNV1 hash of the given byte array and returns it.
        /// </summary>
        /// <param name="array"></param>
        /// <returns></returns>
        public static uint Compute(byte[] array)
        {
            uint value = OFFSET;

            foreach (var b in array)
            {
                value = (value * PRIME) ^ b;
            }

            return value;
        }

        /// <summary>
        /// Computes a hash value for the specified string using Unicode encoding.
        /// </summary>
        /// <remarks>The input string is encoded using UTF-16 (Unicode) before computing the hash. The
        /// result is deterministic for the same input string.</remarks>
        /// <param name="str">The string to compute the hash value for. Cannot be null.</param>
        /// <returns>A 32-bit unsigned integer representing the computed hash value of the input string.</returns>
        public static uint Compute(string str)
        {
            return Compute(Encoding.Unicode.GetBytes(str));
        }
    }

    /// <summary>
    /// Pixel Format for serialization in BTP
    /// </summary>
    internal enum BTPPixelFormat
    {
        PF_Unknown = 0,
        PF_A32B32G32R32F = 1,
        PF_A8R8G8B8 = 2,
        PF_G8 = 3,
        PF_G16 = 4,
        PF_DXT1 = 5,
        PF_DXT3 = 6,
        PF_DXT5 = 7,
        PF_UYVY = 8,
        PF_FloatRGB = 9,
        PF_FloatRGBA = 10,
        PF_DepthStencil = 11,
        PF_ShadowDepth = 12,
        PF_FilteredShadowDepth = 13,
        PF_R32F = 14,
        PF_G16R16 = 15,
        PF_G16R16F = 16,
        PF_G16R16F_FILTER = 17,
        PF_G32R32F = 18,
        PF_A2B10G10R10 = 19,
        PF_A16B16G16R16_UNORM = 20,
        PF_D24 = 21,
        PF_R16F = 22,
        PF_R16F_FILTER = 23,
        PF_BC5 = 24,
        PF_V8U8 = 25,
        PF_A1 = 26,
        PF_NormalMap_LQ = 27,
        PF_NormalMap_HQ = 28,
        PF_A16B16G16R16_FLOAT = 29,
        PF_A16B16G16R16_SNORM = 30,
        PF_FloatR11G11B10 = 31,
        PF_A4R4G4B4 = 32,
        PF_R5G6B5 = 33,
        PF_G8R8 = 34,
        PF_R8_UNORM = 35,
        PF_R8_UINT = 36,
        PF_R8_SINT = 37,
        PF_R16_FLOAT = 38,
        PF_R16_UNORM = 39,
        PF_R16_UINT = 40,
        PF_R16_SINT = 41,
        PF_R8G8_UNORM = 42,
        PF_R8G8_UINT = 43,
        PF_R8G8_SINT = 44,
        PF_R16G16_FLOAT = 45,
        PF_R16G16_UNORM = 46,
        PF_R16G16_UINT = 47,
        PF_R16G16_SINT = 48,
        PF_R32_FLOAT = 49,
        PF_R32_UINT = 50,
        PF_R32_SINT = 51,
        PF_A8 = 52,
        PF_BC7 = 53,
        EPixelFormat_MAX = 54
    };

    /// <summary>
    /// The flags for a mip about how it should be handled in the ASI
    /// </summary>
    [Flags]
    internal enum BTPMipFlags
    {
        /// <summary>
        /// Mip should not be modified
        /// </summary>
        Original = 1 << 1,
        /// <summary>
        /// Mip is located in an external texture file cache
        /// </summary>
        External = 1 << 2,
        /// <summary>
        /// Mip was compressed with Oodle during serialization and must be decompressed to access
        /// the mip data
        /// </summary>
        OodleCompressed = 1 << 3,
    }

    internal sealed class BinaryTexturePackage
    {
        internal const ushort CurrentVersion = 2;

        internal bool LoadMips { get; set; }
        internal bool Verify { get; set; }
        internal bool IsFinalSerialize { get; private set; }
        internal BTPHeader Header { get; }
        internal List<BTPTextureEntry> TextureOverrides { get; set; }
        internal BTPTFCTable TFCTable { get; }

        internal BinaryTexturePackage()
        {
            Header = new BTPHeader(this, null);
            TextureOverrides = new();
            TFCTable = new BTPTFCTable(this, null);
        }

        internal void FinalSerialize(Stream stream)
        {
            IsFinalSerialize = true;
            TFCTable.Serialize(stream);
            Header.TFCTableOffset = TFCTable.TableOffset;
            stream.Seek(0, SeekOrigin.Begin);
            Header.Serialize(stream);
            foreach (var texture in TextureOverrides)
            {
                texture.Serialize(stream);
            }
            IsFinalSerialize = false;
        }
    }

    internal sealed class BTPHeader
    {
        /// <summary>
        /// Magic for BTP
        /// </summary>
        private static string MANIFEST_HEADER => @"LETEXM"; // Must be ASCII

        /// <summary>
        /// BTP owner of this header
        /// </summary>
        public BinaryTexturePackage Owner { get; }

        /// <summary>
        /// Magic string for header
        /// </summary>
        public string Magic { get; set; }

        /// <summary>
        /// Version of serialization for this BTP
        /// </summary>
        public ushort Version { get; set; }

        /// <summary>
        /// FNV1 hash of the DLC folder this BTP is for, combined with the game id
        /// </summary>
        public uint TargetHash { get; set; }

        /// <summary>
        /// Number of texture override entries. Only use during serialization/deserialization.
        /// </summary>
        public uint TextureCount { get; set; }

        /// <summary>
        /// Number of items in the TFC Table. Only use during serialization/deserialization.
        /// </summary>
        public uint TFCTableCount { get; set; }

        /// <summary>
        /// Offset for TFC table
        /// </summary>
        public ulong TFCTableOffset { get; set; }

        /// <summary>
        /// The CRC of the BTP metadata file that is associated with this BTP.
        /// </summary>
        public uint MetadataCRC { get; set; }

        // 16 bytes reserved currently


        public BTPHeader(BinaryTexturePackage owner, Stream btpStream)
        {
            this.Owner = owner;
            if (btpStream != null)
            {
                Deserialize(btpStream);
            }
            else
            {
                // Setup blank object
                Magic = MANIFEST_HEADER;
                Version = BinaryTexturePackage.CurrentVersion;
            }
        }

        /// <summary>
        /// Writes the header out to the given stream.
        /// </summary>
        /// <param name="btpStream">Stream to write to</param>
        public void Serialize(Stream btpStream)
        {
            var headerStartPos = btpStream.Position;
            btpStream.WriteStringASCII(Magic); // Is this null terminated? We dont want that.
            btpStream.WriteUInt16(Version);
            btpStream.WriteUInt32(TargetHash);
            btpStream.WriteUInt32(TextureCount);
            btpStream.WriteUInt32(TFCTableCount);
            btpStream.WriteUInt64(TFCTableOffset);
            btpStream.WriteUInt32(MetadataCRC);
            btpStream.WriteZeros(0x10); // Unused for now

#if DEBUG
            var entrySize = btpStream.Position - headerStartPos;
            if (entrySize != 48)
            {
                throw new Exception(@"Serializer for header produced the wrong size!");
            }
#endif
        }

        private void Deserialize(Stream btpStream)
        {
#if DEBUG
            var entryStart = btpStream.Position;
#endif
            Magic = btpStream.ReadStringASCII(6);
            Version = btpStream.ReadUInt16();
            TargetHash = btpStream.ReadUInt32();
            TextureCount = btpStream.ReadUInt32();
            TFCTableCount = btpStream.ReadUInt32();
            TFCTableOffset = btpStream.ReadUInt64();
            MetadataCRC = btpStream.ReadUInt32();
            btpStream.Skip(0x10); // Reserved - 16 bytes

#if DEBUG
            var entrySize = btpStream.Position - entryStart;
            if (entrySize != 48)
            {
                throw new Exception(@"Deserializer for header produced the wrong size!");
            }
#endif
        }
    }

    /// <summary>
    /// Information about a single texture override in the BTP
    /// </summary>
    [DebuggerDisplay(@"BTPTextureEntry | {OverridePath} | {Mips.Count} mips")]

    internal sealed class BTPTextureEntry
    {
        /// <summary>
        /// Owner of this entry
        /// </summary>
        public BinaryTexturePackage Owner { get; }

        /// <summary>
        /// The memory path to override
        /// </summary>
        public string OverridePath { get; set; }

        /// <summary>
        /// Reference to the BTP TFC Entry
        /// </summary>
        public BTPTFCEntry TFC { get; set; }

        /// <summary>
        /// Texture format for this entry
        /// </summary>
        public BTPPixelFormat Format { get; set; }

        /// <summary>
        /// If texture is sRGB
        /// </summary>
        public bool bSRGB { get; set; }

        /// <summary>
        /// LOD bias to allow higher mips to load
        /// </summary>
        public int InternalFormatLODBias { get; set; } // Stored as byte

        /// <summary>
        /// If texture should never be streamed, package stored must set this
        /// </summary>
        public bool NeverStream { get; set; }

        /// <summary>
        /// Number of actual mips over override is using
        /// </summary>
        public byte PopulatedMipCount { get; set; }

        /// <summary>
        /// List of BTP mips that are populated
        /// </summary>
        public List<BTPMipEntry> Mips { get; } = new(13);

        // Constants
        private const int OVERRIDE_PATH_MAX_CHARS = 255; // +1 for null terminator


        // SERIALIZATION ONLY
        /// <summary>
        /// Where the entry data begins.
        /// </summary>
        private long EntryOffset;

        /// <summary>
        /// Seeks the stream to the start of the entry where it was serialized
        /// </summary>
        /// <param name="btpStream"></param>
        /// <exception cref="Exception"></exception>
        public void SeekTo(Stream btpStream)
        {
            if (EntryOffset == 0)
            {
                throw new Exception(@"BTPStream was incorrectly setup; the offset was never set for an entry!");
            }
            btpStream.Seek(EntryOffset, SeekOrigin.Begin);
        }

        /// <summary>
        /// Constructs a BTP entry with the given owner and deserializes if BTP stream is not null.
        /// </summary>
        /// <param name="owner"></param>
        /// <param name="btpStream"></param>
        public BTPTextureEntry(BinaryTexturePackage owner, Stream btpStream)
        {
            this.Owner = owner;
            if (btpStream != null)
            {
                Deserialize(btpStream);
            }
            else
            {
                // TFC on initialization is always set to None, so that we have a valid reference.
                TFC = Owner.TFCTable.GetTFC(@"None");
                // Allocate mips
                for (int i = 0; i < 13; i++)
                {
                    Mips.Add(new BTPMipEntry(this, null));
                }
            }
        }

        /// <summary>
        /// Serializes the texture entry to the given stream. This does NOT serialize mip data! 
        /// </summary>
        /// <param name="btpStream"></param>
        public void Serialize(Stream btpStream)
        {
            EntryOffset = btpStream.Position;
            btpStream.WritePaddedStringUnicodeNull(OverridePath ?? @"", (OVERRIDE_PATH_MAX_CHARS + 1) * 2); // If not set, we just write a blank string.
            btpStream.WriteInt32(TFC.TableIndex);
            btpStream.WriteInt32((int)Format);
            btpStream.WriteBoolByte(bSRGB);
            btpStream.WriteByte((byte)InternalFormatLODBias);
            btpStream.WriteBoolByte(NeverStream);
            btpStream.WriteByte(PopulatedMipCount);

            // Write out all 13 mip header structs now, from largest to smallest
            for (int i = 0; i < Mips.Count; i++)
            {
                var mip = Mips[i];
                mip.Serialize(btpStream, i >= PopulatedMipCount);
            }

            if (Owner.IsFinalSerialize
                && (Format == BTPPixelFormat.PF_Unknown || PopulatedMipCount is < 1 or > 13
                    || Mips.Any(mip => (mip.Flags & BTPMipFlags.External) != 0) && TFC.TableIndex == 0))
                throw new InvalidDataException("M3TO texture metadata is invalid for final serialization.");

#if DEBUG
            var entrySize = btpStream.Position - EntryOffset;
            if (entrySize != 836)
            {
                throw new Exception(@"Serializer for texture produced the wrong size!");
            }

            if (Owner.IsFinalSerialize)
            {
                if (Format == BTPPixelFormat.PF_Unknown)
                {
                    throw new Exception(@"Serializer for texture reports no format!");
                }

                if (PopulatedMipCount == 0)
                {
                    throw new Exception(@"Serializer for texture reports 0 mips!");
                }

                if (Mips.Any(x => (x.Flags & BTPMipFlags.External) != 0) && TFC.TableIndex == 0)
                {
                    throw new Exception(@"Serializer reports TFC table index is zero but there are external mips.");
                }
            }
#endif
        }


        /// <summary>
        /// Populates this object from the given stream
        /// </summary>
        /// <param name="btpStream">Stream to read from</param>
        private void Deserialize(Stream btpStream)
        {
            EntryOffset = btpStream.Position;
            OverridePath = btpStream.ReadStringUnicodeNull();
            btpStream.Seek(EntryOffset + ((OVERRIDE_PATH_MAX_CHARS + 1) * 2), SeekOrigin.Begin); // +1 for null terminator
            var tfcTableIndex = btpStream.ReadInt32();
            TFC = Owner.TFCTable.TFCTable.Values.FirstOrDefault(x => x.TableIndex == tfcTableIndex);
            if (TFC == null)
            {
                throw new Exception($@"Invalid TFC table index in BTP: {tfcTableIndex}");
            }
            Format = (BTPPixelFormat)btpStream.ReadInt32();
            bSRGB = btpStream.ReadBoolByte();
            InternalFormatLODBias = btpStream.ReadByte();
            NeverStream = btpStream.ReadBoolByte();

            // Read mip structs, even unpopulated
            PopulatedMipCount = (byte)btpStream.ReadByte();
            for (var i = 0; i < 13; i++)
            {
                var mip = new BTPMipEntry(this, btpStream, i >= PopulatedMipCount);
                Mips.Add(mip);
            }

#if DEBUG
            var entrySize = btpStream.Position - EntryOffset;
            if (entrySize != 836)
            {
                throw new Exception(@"Deserializer for texture produced the wrong size!");
            }
#endif
        }
    }

    /// <summary>
    /// Information about a mip in the BTP
    /// </summary>
    [DebuggerDisplay(@"BTPMipEntry {Width}x{Height}, UC: {UncompressedSize}, CS: {CompressedSize}, Flags: {Flags}")]
    internal sealed class BTPMipEntry
    {
        /// <summary>
        /// Texture override owner of this mip
        /// </summary>
        private BTPTextureEntry Owner { get; }

        /// <summary>
        /// Size of data uncompressed
        /// </summary>
        public int UncompressedSize { get; set; }
        /// <summary>
        /// Size of compressed data (if compressed)
        /// </summary>
        public int CompressedSize { get; set; }
        /// <summary>
        /// Offset to data, either in the TFC or in the BTP if this is package stored in flags
        /// </summary>
        public long CompressedOffset { get; set; }

        /// <summary>
        /// Width of the mip
        /// </summary>
        public short Width { get; set; }
        /// <summary>
        /// Height of the mip
        /// </summary>
        public short Height { get; set; }

        /// <summary>
        /// Flags of the mip - storage type, etc
        /// </summary>
        public BTPMipFlags Flags { get; set; }

        /// <summary>
        /// Data for the mip. This will be null the owner has LoadMips = false.
        /// </summary>
        public byte[] Data { get; set; }

        /// <summary>
        /// Offset of the entry when serialized/deserialized
        /// </summary>
        public long EntryOffset { get; set; }

        /// <summary>
        /// Seeks the stream to the start of the entry where it was serialized
        /// </summary>
        /// <param name="btpStream"></param>
        /// <exception cref="Exception"></exception>
        public void SeekTo(Stream btpStream)
        {
            if (EntryOffset == 0)
            {
                throw new Exception(@"BTPStream was incorrectly setup; the offset was never set for a mip entry!");
            }
            btpStream.Seek(EntryOffset, SeekOrigin.Begin);
        }

        public BTPMipEntry(BTPTextureEntry owner, Stream btpStream, bool isUnused = false)
        {
            Owner = owner;
            if (btpStream != null)
            {
                Deserialize(btpStream, isUnused);
            }
        }

        /// <summary>
        /// Serializes the mip header to the stream. This does NOT serialize data!
        /// </summary>
        /// <param name="btpStream">Stream to serialize to</param>
        public void Serialize(Stream btpStream, bool isUnused, bool forceTest = false)
        {
            EntryOffset = btpStream.Position;
            btpStream.WriteInt32(UncompressedSize);
            btpStream.WriteInt32(CompressedSize);
            btpStream.WriteInt64(CompressedOffset);
            btpStream.WriteInt16(Width);
            btpStream.WriteInt16(Height);
            btpStream.WriteInt32((int)Flags);

            bool invalidStorage = UncompressedSize <= 0 || CompressedSize <= 0 || CompressedOffset <= 0;
            bool invalidDimensions = Width is < 1 or > 4096 || Height is < 1 or > 4096;
            if (!isUnused && (forceTest && invalidStorage
                || Owner.Owner.IsFinalSerialize && (invalidStorage || invalidDimensions)))
                throw new InvalidDataException("M3TO mip metadata is invalid for final serialization.");

#if DEBUG
            var entrySize = btpStream.Position - EntryOffset;
            if (entrySize != 24)
            {
                throw new Exception(@"Serializer for mip produced the wrong size!");
            }

            if ((Owner.Owner.IsFinalSerialize || forceTest) && !isUnused)
            {
                if (UncompressedSize == 0)
                {
                    throw new Exception(@"Uncompressed size is 0 on final serialize!");
                }

                if (CompressedSize == 0)
                {
                    throw new Exception(@"Compressed size is 0 on final serialize!");
                }

                if (CompressedOffset == 0)
                {
                    throw new Exception(@"Compressed offset is 0 on final serialize!");
                }
            }
#endif
        }

        /// <summary>
        /// Serializes the data of this mip to the stream, as well as updating the stream's data for offset and sizing. 
        /// This also updates the BTP object to reflect the new data.
        /// </summary>
        /// <param name="btpStream">Stream to serialize to.</param>
        /// <param name="offset">The offset of the data. If data is present, it will be written at this position. If new data, this should be at the end of the stream</param>
        /// <param name="data">If null, this data is already in the stream, and we shouldn't write it again.</param>
        public void SerializeData(Stream btpStream, long offset, int compressedSize, byte[] data = null)
        {
            // We may be writing a deduplicated mip;
            // in this case we only set the offset
            // and compressed size, as everything else
            // should be identical to whatever we are
            // pointing at (since it's a duplicate).

            // Write package stored data to BTP
            if (data != null)
            {
                btpStream.Seek(offset, SeekOrigin.Begin);
                btpStream.Write(data);
                CompressedSize = compressedSize;
            }

#if DEBUG
            if (compressedSize == 0)
            {
                throw new Exception(@"Compressed size is 0!");
            }

            // Offset in TFC and package should never be zero since GUID in TFC and BTP header
            if (offset == 0)
            {
                throw new Exception(@"Offset is 0!");
            }
#endif

            // The offset where data resides (in BTP or in TFC)
            CompressedOffset = offset;
            CompressedSize = compressedSize;

            // Return to entry
            SeekTo(btpStream);

            // Rewrite entry data with updated information.
            Serialize(btpStream, false, true);
        }

        /// <summary>
        /// Populates this object from the given stream
        /// </summary>
        /// <param name="btpStream">STream to read from</param>
        private void Deserialize(Stream btpStream, bool isUnused)
        {
            EntryOffset = btpStream.Position;
            UncompressedSize = btpStream.ReadInt32();
            CompressedSize = btpStream.ReadInt32();
            CompressedOffset = btpStream.ReadInt64();
            Width = btpStream.ReadInt16();
            Height = btpStream.ReadInt16();
            Flags = (BTPMipFlags)btpStream.ReadInt32();

#if DEBUG
            var entrySize = btpStream.Position - EntryOffset;
            if (entrySize != 24)
            {
                throw new Exception(@"Deserializer for mip produced the wrong size!");
            }

            if (!isUnused)
            {
                if (CompressedSize == 0)
                {
                    throw new Exception(@"Deserializer for mip reports 0 compressed size!");
                }

                if (UncompressedSize == 0)
                {
                    throw new Exception(@"Deserializer for mip reports 0 uncompressed size!");
                }

                if (Width == 0 || Height == 0)
                {
                    throw new Exception(@"Deserializer for mip reports 0 Width/Height!");
                }

                if (Width > 8192 || Width < 0)
                {
                    throw new Exception($@"Deserializer for mip reports width out of bounds: {Width}");
                }

                if (Height > 8192 || Height < 0)
                {
                    throw new Exception($@"Deserializer for mip reports height out of bounds: {Height}");
                }
            }
#endif

            if (!isUnused && Owner.Owner.LoadMips && (Flags & BTPMipFlags.External) == 0)
            {
                var data = LoadData(btpStream);
                // If not in verify mode, set the mip data value
                if (!Owner.Owner.Verify)
                {
                    Data = data;
                }
            }
        }

        private byte[] LoadData(Stream btpStream)
        {
            // Local stored mip
            // To use it's data as a texture
            // you must check for oodle compression flag.
            var mipEntryEnd = btpStream.Position;
            btpStream.Seek(CompressedOffset, SeekOrigin.Begin);
            if (btpStream.Position + CompressedSize > btpStream.Length)
            {
                throw new Exception($@"The requested mip data is out of bounds of the BTP file. BTP Size: {btpStream.Length}, Requested Offset {CompressedOffset}, Requested size: {CompressedSize}");
            }
            var data = Data = btpStream.ReadToBuffer(CompressedSize);

            // Verify decompression
            byte[] decompressedData = null;
            if ((Flags & BTPMipFlags.OodleCompressed) != 0)
            {
                decompressedData = new byte[UncompressedSize];
                OodleHelper.Decompress(data, decompressedData);
            }

            // Return
            btpStream.Seek(mipEntryEnd, SeekOrigin.Begin);

            return decompressedData;
        }
    }

    /// <summary>
    /// Information about a TFC in the BTP's TFC table
    /// </summary>
    internal sealed class BTPTFCEntry
    {
        /// <summary>
        /// Table owner of this TFC Entry
        /// </summary>
        private BTPTFCTable Owner;

        /// <summary>
        /// Name for no TFC association.
        /// </summary>
        public const string NO_TFC_NAME = @"None";
        /// <summary>
        /// Maximum length of a TFC name string.
        /// </summary>
        public const int TFC_NAME_MAX_SIZE = 63; // 128 bytes reserved, stored as 2byte unicode per character, so 64 chars -1 for null terminator.

        /// <summary>
        /// Constructor for BTPTFCEntry
        /// </summary>
        /// <param name="owner">Owning BTP</param>
        public BTPTFCEntry(BTPTFCTable owner, Stream btpStream)
        {
            Owner = owner;
            if (btpStream != null)
            {
                Deserialize(btpStream);
            }
        }

        /// <summary>
        /// Name of the TFC. "No TFC" uses the name 'None'.
        /// </summary>
        public string TFCName { get; internal set; } // None if no TFC

        /// <summary>
        /// Guid of the TFC. "No TFC" uses the Guid of all zero's.
        /// </summary>
        public Guid TFCGuid { get; internal set; } // 0 if no TFC

        /// <summary>
        /// Index into BTP table.
        /// </summary>
        public int TableIndex { get; internal set; }

        /// <summary>
        /// Offset of this entry in the stream when serialized/deserialized
        /// </summary>
        private long EntryOffset;

        /// <summary>
        /// Serializes this TFC entry to the given stream.
        /// </summary>
        /// <param name="btpStream"></param>
        internal void Serialize(Stream btpStream)
        {
            EntryOffset = btpStream.Position;
            btpStream.WritePaddedStringUnicodeNull(TFCName, (TFC_NAME_MAX_SIZE + 1) * 2);
            btpStream.WriteGuid(TFCGuid);

#if DEBUG
            var entrySize = btpStream.Position - EntryOffset;
            if (entrySize != 144)
            {
                throw new Exception(@"Serialize for TFC entry produced the wrong size!");
            }
#endif
        }


        /// <summary>
        /// Populates this object from the given stream.
        /// </summary>
        /// <param name="btpStream"></param>
        /// <exception cref="NotImplementedException"></exception>
        private void Deserialize(Stream btpStream)
        {
            EntryOffset = btpStream.Position;
            TFCName = btpStream.ReadStringUnicodeNull();
            btpStream.Seek(EntryOffset + (TFC_NAME_MAX_SIZE + 1) * 2, SeekOrigin.Begin);
            TFCGuid = btpStream.ReadGuid();
            // Index must be assigned by the caller
        }
    }

    internal sealed class BTPTFCTable
    {
        /// <summary>
        /// Owner of this BTP TFC Table
        /// </summary>
        public BinaryTexturePackage Owner { get; }

        /// <summary>
        /// Location of the table in the stream
        /// </summary>
        internal ulong TableOffset { get; set; }

        /// <summary>
        /// Next TFC Index to assign in the table when adding another TFC table item
        /// </summary>
        int NextTFCIndex = 0;

        /// <summary>
        /// Map of TFC name to their entries. Used for performance.
        /// </summary>
        internal CaseInsensitiveDictionary<BTPTFCEntry> TFCTable;

        public BTPTFCTable(BinaryTexturePackage owner, Stream btpStream)
        {
            Owner = owner;
            if (btpStream != null)
            {
                Deserialize(btpStream);
            }
            else
            {
                TFCTable = new();
            }
        }

        /// <summary>
        /// Serializes the TFC table to the given stream, at the end.
        /// </summary>
        /// <param name="btpStream"></param>
        internal void Serialize(Stream btpStream)
        {
            // Align to 16 bytes
            btpStream.SeekEnd();
            while (btpStream.Position % 16 != 0)
                btpStream.WriteByte(0);

            // Store table offset and write the entries
            TableOffset = (ulong)btpStream.Position;
            foreach (var btpTFC in TFCTable.Values.OrderBy(x => x.TableIndex))
            {
                btpTFC.Serialize(btpStream);
            }
        }

        private void Deserialize(Stream btpStream)
        {
            TableOffset = (ulong)btpStream.Position;
            TFCTable = new();
            for (int i = 0; i < Owner.Header.TFCTableCount; i++)
            {
                var entry = new BTPTFCEntry(this, btpStream);
                entry.TableIndex = i;
                TFCTable[entry.TFCName] = entry;
                NextTFCIndex++; // Increment to make sure it's accurate if we edit this
            }
        }


        /// <summary>
        /// Updates the building TFC table
        /// </summary>
        /// <param name="tfc">TFC name</param>
        /// <param name="guid">TFC Guid</param>
        /// <returns></returns>
        internal int GetTFCTableIndex(string tfc, Guid guid, ExportEntry export)
        {
            if (guid == Guid.Empty && tfc != @"None")
            {
                throw new InvalidDataException($"BTP references unknown TFC '{tfc}'.");
            }

            if (TFCTable.TryGetValue(tfc, out var tfcInfo))
            {
                if (tfcInfo.TFCGuid != guid)
                {
                    throw new InvalidDataException($"TFC GUID mismatch for '{tfc}'.");
                }

                return tfcInfo.TableIndex;
            }

            // Insert new one
            if (tfc.Length > BTPTFCEntry.TFC_NAME_MAX_SIZE)
            {
                throw new InvalidDataException($"TFC name '{tfc}' exceeds the BTP format limit.");
            }

            tfcInfo = new BTPTFCEntry(this, null)
            {
                TableIndex = NextTFCIndex++,
                TFCGuid = guid,
                TFCName = tfc
            };

            TFCTable[tfc] = tfcInfo;
            Owner.Header.TFCTableCount++;
            return tfcInfo.TableIndex;
        }

        /// <summary>
        /// Returns the TFC entry for the given TFC name. Does not create the TFC entry.
        /// </summary>
        /// <param name="tfcName"></param>
        /// <returns></returns>
        internal BTPTFCEntry GetTFC(string tfcName)
        {
            if (tfcName == null)
            {
                // This will always be popuplated
                return TFCTable[@"None"];
            }

            return TFCTable[tfcName];
        }

        /// <summary>
        /// Returns TFC entry for the given TFC name (if null, it returns the None entry). Creates if not found.
        /// </summary>
        /// <param name="tfcName"></param>
        /// <param name="guid"></param>
        /// <param name="export"></param>
        /// <returns></returns>
        internal BTPTFCEntry GetOrAddTFC(string tfcName, Guid guid, ExportEntry export)
        {
            if (tfcName != null)
            {
                GetTFCTableIndex(tfcName, guid, export);
            }
            return GetTFC(tfcName);
        }
    }
}
