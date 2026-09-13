using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;

namespace Deployd.Mele;

internal sealed record ConfigValue(string Key, string Value);
internal sealed record ConfigSection(string Name, List<ConfigValue> Values);
internal sealed record ConfigFile(string Name, List<ConfigSection> Sections)
{
    internal string Basename => Name.Replace('\\', '/').Split('/').Last();
}

internal sealed class Le1Config
{
    internal const int Limit = 64 * 1024 * 1024;
    private static readonly Encoding Unicode = new UnicodeEncoding(false, false, true);
    internal List<ConfigFile> Files { get; } = new();

    internal static Le1Config Read(Stream stream)
    {
        if (stream.Length > Limit)
            throw new InvalidDataException("LE1 configuration exceeds 64 MiB.");
        using var reader = new BinaryReader(stream, Unicode, leaveOpen: true);
        int Count(int maximum)
        {
            int count = reader.ReadInt32();
            if (count < 0 || count > maximum || count > (stream.Length - stream.Position) / 4)
                throw new InvalidDataException("Invalid LE1 configuration count.");
            return count;
        }
        string Text()
        {
            int length = reader.ReadInt32();
            if (length == 0)
                return string.Empty;
            if (length >= 0 || length < -1024 * 1024 || -(long)length * 2 > stream.Length - stream.Position)
                throw new InvalidDataException("Invalid LE1 configuration string length.");
            byte[] bytes = reader.ReadBytes(-length * 2);
            if (bytes[^1] != 0 || bytes[^2] != 0)
                throw new InvalidDataException("LE1 configuration string is not terminated.");
            string value = Unicode.GetString(bytes, 0, bytes.Length - 2);
            if (value.Contains('\0'))
                throw new InvalidDataException("Embedded null in LE1 configuration.");
            return value;
        }
        var config = new Le1Config();
        var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        int files = Count(256);
        if (files == 0)
            throw new InvalidDataException("LE1 configuration must contain assets.");
        for (int fileIndex = 0; fileIndex < files; fileIndex++)
        {
            var file = new ConfigFile(Text(), new List<ConfigSection>());
            if (string.IsNullOrWhiteSpace(file.Basename) || !names.Add(file.Basename))
                throw new InvalidDataException("Empty or ambiguous LE1 configuration asset.");
            config.Files.Add(file);
            int sections = Count(100000);
            for (int sectionIndex = 0; sectionIndex < sections; sectionIndex++)
            {
                var section = new ConfigSection(Text(), new List<ConfigValue>());
                file.Sections.Add(section);
                int values = Count(1000000);
                for (int valueIndex = 0; valueIndex < values; valueIndex++)
                    section.Values.Add(new ConfigValue(Text(), Text()));
            }
        }
        if (stream.Position != stream.Length)
            throw new InvalidDataException("Unexpected trailing LE1 configuration data.");
        return config;
    }

    internal MemoryStream Serialize()
    {
        var stream = new MemoryStream();
        try
        {
            using var writer = new BinaryWriter(stream, Unicode, leaveOpen: true);
            void Text(string value)
            {
                if (value.Length >= 1024 * 1024 || stream.Length + 2L * value.Length + 6 > Limit)
                    throw new InvalidDataException("Generated LE1 configuration exceeds its size limit.");
                writer.Write(value.Length == 0 ? 0 : -(value.Length + 1));
                if (value.Length != 0)
                {
                    writer.Write(Unicode.GetBytes(value));
                    writer.Write((ushort)0);
                }
            }
            writer.Write(Files.Count);
            foreach (var file in Files)
            {
                Text(file.Name);
                writer.Write(file.Sections.Count);
                foreach (var section in file.Sections)
                {
                    Text(section.Name);
                    writer.Write(section.Values.Count);
                    foreach (var entry in section.Values)
                    {
                        Text(entry.Key);
                        Text(entry.Value);
                    }
                }
            }
            stream.Position = 0;
            if (!SameAs(Read(stream)))
                throw new InvalidDataException("LE1 configuration changed during serialization.");
            stream.Position = 0;
            return stream;
        }
        catch
        {
            stream.Dispose();
            throw;
        }
    }

    internal bool SameAs(Le1Config other) => Files.Count == other.Files.Count
        && Files.Zip(other.Files).All(files => files.First.Name == files.Second.Name
            && files.First.Sections.Count == files.Second.Sections.Count
            && files.First.Sections.Zip(files.Second.Sections).All(sections => sections.First.Name == sections.Second.Name
                && sections.First.Values.SequenceEqual(sections.Second.Values)));
}
