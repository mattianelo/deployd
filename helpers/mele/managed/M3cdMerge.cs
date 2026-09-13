using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Threading;

namespace Deployd.Mele;

internal sealed record ConfigEdit(string File, string Section, string Key, char Action, string Value);

internal static class M3cdMerge
{
    internal static bool IsTarget(string name) => name.Length == 17
        && name.StartsWith("Coalesced_", StringComparison.OrdinalIgnoreCase)
        && name.EndsWith(".bin", StringComparison.OrdinalIgnoreCase)
        && name.AsSpan(10, 3).ToArray().All(char.IsAsciiLetter);

    internal static bool IsManifest(string name) => name.StartsWith("ConfigDelta-", StringComparison.OrdinalIgnoreCase)
        && name.EndsWith(".m3cd", StringComparison.OrdinalIgnoreCase) && name.Length > 17;

    private static bool Identifier(string value) => value.Length is > 0 and <= 1024
        && value.All(character => char.IsAsciiLetterOrDigit(character) || character is '_' or '.');

    internal static ConfigEdit[] Parse(Stream stream)
    {
        if (stream.Length > 1024 * 1024)
            throw new InvalidDataException("M3CD exceeds 1 MiB.");
        using var reader = new StreamReader(stream, new UTF8Encoding(false, true), detectEncodingFromByteOrderMarks: false, leaveOpen: true);
        var edits = new List<ConfigEdit>();
        string file = null;
        string section = null;
        foreach (string input in reader.ReadToEnd().Split('\n'))
        {
            string line = input.Trim();
            if (line.Length == 0 || line.StartsWith(';'))
                continue;
            if (line.Any(character => char.IsControl(character) && character != '\t'))
                throw new InvalidDataException("Control character in M3CD.");
            if (line.StartsWith('['))
            {
                if (!line.EndsWith(']'))
                    throw new InvalidDataException("Unterminated M3CD section.");
                string header = line[1..^1];
                int split = header.IndexOf(' ');
                if (split < 0)
                    throw new InvalidDataException("M3CD sections require an INI filename and section name.");
                file = header[..split];
                section = header[(split + 1)..];
                if (!Identifier(file) || !file.EndsWith(".ini", StringComparison.OrdinalIgnoreCase)
                    || !Identifier(section) || file.StartsWith('.') || section.StartsWith('.'))
                    throw new InvalidDataException("Unsupported M3CD asset or section name.");
                continue;
            }
            int separator = line.IndexOf('=');
            if (file is null || separator <= 0)
                throw new InvalidDataException("M3CD entry must follow a section and contain a key/value pair.");
            string key = line[..separator].Trim();
            char action = "+-.!>".Contains(key[0]) ? key[0] : '.';
            if ("+-.!>".Contains(key[0]))
                key = key[1..];
            if (!Identifier(key) || "+-.!>".Contains(key[0]) || key.Contains("..", StringComparison.Ordinal))
                throw new InvalidDataException("Unsupported M3CD key or double-typed operation.");
            edits.Add(new ConfigEdit(file, section, key, action, line[(separator + 1)..].Trim()));
            if (edits.Count > 16384)
                throw new InvalidDataException("M3CD has too many operations.");
        }
        if (edits.Count == 0)
            throw new InvalidDataException("M3CD contains no operations.");
        return edits.ToArray();
    }

    internal static void Apply(Le1Config config, IEnumerable<ConfigEdit> edits)
    {
        foreach (var edit in edits)
        {
            var file = config.Files.SingleOrDefault(file => file.Basename.Equals(edit.File, StringComparison.OrdinalIgnoreCase))
                ?? throw new InvalidDataException("M3CD names an asset absent from the original configuration.");
            var matches = file.Sections.Where(section => section.Name.Equals(edit.Section, StringComparison.OrdinalIgnoreCase)).ToArray();
            if (matches.Length > 1)
                throw new InvalidDataException("M3CD target section is ambiguous.");
            var section = matches.SingleOrDefault();
            if (section is null)
            {
                section = new ConfigSection(edit.Section, new List<ConfigValue>());
                file.Sections.Add(section);
            }
            bool Matches(ConfigValue entry) => entry.Key.Equals(edit.Key, StringComparison.OrdinalIgnoreCase);
            switch (edit.Action)
            {
                case '-':
                    section.Values.RemoveAll(entry => Matches(entry) && entry.Value == edit.Value);
                    break;
                case '!':
                    section.Values.RemoveAll(entry => Matches(entry));
                    break;
                case '>':
                    section.Values.RemoveAll(entry => Matches(entry));
                    section.Values.Add(new ConfigValue(edit.Key, edit.Value));
                    break;
                case '+':
                    if (!section.Values.Any(entry => Matches(entry) && entry.Value == edit.Value))
                        section.Values.Add(new ConfigValue(edit.Key, edit.Value));
                    break;
                case '.':
                    section.Values.Add(new ConfigValue(edit.Key, edit.Value));
                    break;
                default:
                    throw new InvalidDataException("Unknown M3CD operation.");
            }
        }
    }

    internal static OutputFile[] Execute(MergeRequest request, CancellationToken cancellation, Action<int, int> progress)
    {
        TransformProtocol.Validate(request);
        if (request.Operation != "le1-m3cd")
            throw new InvalidDataException("M3CD requires its own transformation operation.");
        var targets = new List<(string Path, Le1Config Config)>();
        foreach (var target in request.Targets)
        {
            cancellation.ThrowIfCancellationRequested();
            using var original = TransformProtocol.ReadVerified(request.OriginalRoot, target.Original, Le1Config.Limit);
            using var current = TransformProtocol.ReadVerified(request.InputRoot, target.Current, Le1Config.Limit);
            Le1Config.Read(current);
            targets.Add((target.Current.Path, Le1Config.Read(original)));
        }
        var ordered = request.Contributions.OrderBy(item => item.Mount)
            .ThenBy(item => item.Manifest.Path, StringComparer.OrdinalIgnoreCase).ToArray();
        int completed = 0;
        foreach (var contribution in ordered)
        {
            cancellation.ThrowIfCancellationRequested();
            using var stream = TransformProtocol.ReadVerified(request.InputRoot, contribution.Manifest, 1024 * 1024);
            var edits = Parse(stream);
            foreach (var target in targets)
            {
                cancellation.ThrowIfCancellationRequested();
                Apply(target.Config, edits);
            }
            progress(++completed, ordered.Length);
        }
        var outputs = new List<OutputFile>();
        foreach (var target in targets.OrderBy(target => target.Path, StringComparer.Ordinal))
        {
            cancellation.ThrowIfCancellationRequested();
            using var serialized = target.Config.Serialize();
            cancellation.ThrowIfCancellationRequested();
            string hash = Convert.ToHexStringLower(SHA256.HashData(serialized));
            serialized.Position = 0;
            string path = Path.Combine(request.OutputRoot, target.Path);
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            TransformProtocol.ValidateRoot(Path.GetDirectoryName(path));
            using var output = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None);
            serialized.CopyTo(output);
            output.Flush(flushToDisk: true);
            outputs.Add(new OutputFile(target.Path, serialized.Length, hash));
        }
        return outputs.ToArray();
    }
}
