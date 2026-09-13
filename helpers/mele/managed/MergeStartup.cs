using System;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;

using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Unreal;
using LegendaryExplorerCore.Unreal.BinaryConverters;
using LegendaryExplorerCore.UnrealScript;

namespace Deployd.Mele;

internal static class MergeStartup
{
    internal static IMEPackage Build(string resolution, ScriptPackageCache cache, OutfitSelection[] outfits,
        EmailMerge[] emails, CancellationToken cancellation)
    {
        var package = MEPackageHandler.CreateMemoryEmptyPackage(
            Path.Combine(resolution, "BioGame", "CookedPCConsole", MergeDlcConfig.Startup), MEGame.LE2);
        try
        {
            cancellation.ThrowIfCancellationRequested();
            // Native compiler types have no script exports to resolve in an empty package.
            var core = ExportCreator.CreatePackageImport(package, "Core");
            foreach (string name in new[] { "Package", "Function", "State", "ScriptStruct", "Enum", "Const", "ObjectReferencer",
                "ByteProperty", "IntProperty", "BoolProperty", "FloatProperty", "ClassProperty", "ComponentProperty",
                "ObjectProperty", "NameProperty", "DelegateProperty", "InterfaceProperty", "StructProperty", "StrProperty",
                "MapProperty", "StringRefProperty", "ArrayProperty" })
                package.AddImport(new ImportEntry(package, core, name) { ClassName = "Class", PackageFile = "Core" });
            var options = new UnrealScriptOptionsPackage { Cache = cache, GamePathOverride = resolution,
                CustomFileResolver = (name, _) => cache.ResolveCandidate(name) };
            using var library = new FileLib(package, useAutoReinitialization: false);
            if (!library.Initialize(options, canUseBinaryCache: false))
                throw new InvalidDataException($"Merge DLC compiler initialization failed: {library.InitializationLog}");
            var source = new StringBuilder("class BioAutoConditionals extends BioConditionals;\n");
            foreach (var selection in outfits)
            {
                cancellation.ThrowIfCancellationRequested();
                Function(source, selection.Outfit.Conditional,
                    $"local BioGlobalVariableTable gv; gv = bioWorld.GetGlobalVariables(); return gv.GetInt({SquadStreaming.PlotId(MEGame.LE2, selection.Outfit.HenchName)}) == {selection.Value};");
            }
            foreach (var email in emails)
            {
                cancellation.ThrowIfCancellationRequested();
                Function(source, email.Conditional, string.IsNullOrWhiteSpace(email.Trigger)
                    ? $"local BioGlobalVariableTable gv; gv = bioWorld.GetGlobalVariables(); return gv.GetInt({email.Status}) == 0;"
                    : email.Trigger);
            }
            string classPath = "PlotManager" + MergeDlcConfig.Name + ".BioAutoConditionals";
            M3mClasses.Apply(package, classPath, source.ToString(), new System.Collections.Generic.HashSet<string>(), library, options);
            var statePackage = ExportCreator.CreatePackageExport(package, "PlotManagerAuto" + MergeDlcConfig.Name, cache: cache);
            var stateExport = ExportCreator.CreateExport(package, "StateTransitionMap", "BioStateEventMap", statePackage, indexed: false, cache: cache);
            if (stateExport.ClassName != "BioStateEventMap")
                throw new InvalidDataException("Generated state transitions could not resolve their game class.");
            var states = BioStateEventMap.Create();
            foreach (var email in emails)
            {
                states.StateEvents.Add(new BioStateEventMap.BioStateEvent {
                    ID = email.Transition,
                    Elements = new() {
                        new BioStateEventMap.BioStateEventElementBool { GlobalBool = 4328, NewState = true, Type = BioStateEventMap.BioStateEventElementType.Bool },
                        new BioStateEventMap.BioStateEventElementBool { GlobalBool = 4321, NewState = true, Type = BioStateEventMap.BioStateEventElementType.Bool },
                        new BioStateEventMap.BioStateEventElementInt { GlobalInt = email.Status, NewValue = 1, InstanceVersion = 1, Type = BioStateEventMap.BioStateEventElementType.Int },
                    },
                });
            }
            stateExport.ObjectFlags |= UnrealFlags.EObjectFlags.Public | UnrealFlags.EObjectFlags.Standalone;
            stateExport.WriteBinary(states);
            var referencer = ExportCreator.CreateExport(package, "ObjectReferencer", "ObjectReferencer", cache: cache);
            if (referencer.ClassName != "ObjectReferencer")
                throw new InvalidDataException("Generated startup references could not resolve their native class.");
            referencer.ObjectFlags |= UnrealFlags.EObjectFlags.Public | UnrealFlags.EObjectFlags.Standalone;
            var conditional = package.FindExport(classPath) ?? throw new InvalidDataException("Generated conditional class is missing.");
            referencer.WriteProperty(new ArrayProperty<ObjectProperty>(new[] { new ObjectProperty(conditional), new ObjectProperty(stateExport) }, "ReferencedObjects"));
            if (package.Exports.Count(entry => entry.Parent == conditional && entry.ClassName == "Function") != outfits.Length + emails.Length)
                throw new InvalidDataException("Generated conditional function count is inconsistent.");
            cancellation.ThrowIfCancellationRequested();
            return package;
        }
        catch { package.Dispose(); throw; }
    }

    private static void Function(StringBuilder source, int id, string body) => source.Append("public function bool F").Append(id)
        .Append("(BioWorldInfo bioWorld, int Argument) {\n").Append(body).Append("\n}\n");
}
