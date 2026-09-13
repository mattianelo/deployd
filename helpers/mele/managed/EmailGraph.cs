using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;

using LegendaryExplorerCore.Kismet;
using LegendaryExplorerCore.Packages;
using LegendaryExplorerCore.Packages.CloningImportingAndRelinking;
using LegendaryExplorerCore.Unreal;

namespace Deployd.Mele;

internal static class EmailGraph
{
    private const string Root = "TheWorld.PersistentLevel.Main_Sequence.";

    private static ExportEntry Find(IMEPackage package, string path) => package.FindExport(Root + path)
        ?? throw new InvalidDataException($"Email merge requires the game sequence '{path}'.");

    private static ExportEntry[] Children(ExportEntry sequence) => KismetHelper.GetSequenceObjects(sequence).OfType<ExportEntry>().ToArray();

    private static ExportEntry One(ExportEntry sequence, string type)
    {
        var matches = Children(sequence).Where(entry => entry.ClassName == type).ToArray();
        if (matches.Length != 1) throw new InvalidDataException($"Email sequence '{sequence.ObjectName}' requires one {type} node.");
        return matches[0];
    }

    private static ExportEntry Next(ExportEntry node)
    {
        var links = KismetHelper.GetOutputLinksOfNode(node);
        if (links.Count != 1 || links[0].Count != 1 || links[0][0].InputLinkIdx != 0 || links[0][0].LinkedOp is not ExportEntry next)
            throw new InvalidDataException("Email sequence has an unsupported continuation.");
        return next;
    }

    private static void Connect(ExportEntry source, ExportEntry target, string pin = "Out")
    {
        KismetHelper.RemoveOutputLinks(source);
        KismetHelper.CreateOutputLink(source, pin, target);
    }

    private static ExportEntry Clone(ExportEntry donor, ExportEntry parent, string name)
    {
        var clone = EntryCloner.CloneTree(donor);
        clone.ObjectName = new NameReference(name);
        KismetHelper.AddObjectToSequence(clone, parent);
        KismetHelper.RemoveOutputLinks(clone);
        return clone;
    }

    private static void Plot(ExportEntry node, int id)
    {
        node.WriteProperty(new IntProperty(id, "m_nIndex"));
        node.WriteProperty(new IntProperty(-1, "m_nPrevRegionIndex"));
        node.WriteProperty(new IntProperty(-1, "m_nPrevPlotIndex"));
    }

    private static ExportEntry Linked(ExportEntry node, string pin)
    {
        var links = KismetHelper.GetVariableLinksOfNode(node).Where(link => link.LinkDesc == pin).ToArray();
        if (links.Length != 1 || links[0].LinkedNodes.Count != 1 || links[0].LinkedNodes[0] is not ExportEntry linked)
            throw new InvalidDataException($"Email node lacks its '{pin}' variable.");
        return linked;
    }

    private static void StringRef(ExportEntry node, int value)
    {
        string property = node.ClassName switch
        {
            "BioSeqVar_StrRefLiteral" => "m_srStringID",
            "BioSeqVar_StrRef" => "m_srValue",
            _ => throw new InvalidDataException("Email title or description is not a string reference."),
        };
        node.WriteProperty(new StringRefProperty(value, property));
    }

    internal static void Apply(IMEPackage package, EmailMerge[] emails, CancellationToken cancellation)
    {
        if (package.Game != MEGame.LE2 || emails.Length is < 1 or > 4096) throw new InvalidDataException("Email graph merging requires LE2 and bounded contributions.");
        var send = Find(package, "Send_Messages");
        var read = Find(package, "Mark_Read");
        var display = Find(package, "Display_Messages");
        var archive = Find(package, "Archive_Message");
        var sendDonor = Find(package, "Send_Messages.DLC_CER");
        var boolDonor = Find(package, "Send_Messages.DLC_CER_Arc");
        var readDonor = Find(package, "Mark_Read.DLC_CER");
        var displayDonor = Find(package, "Display_Messages.Display_DLC_CER");
        var terminals = Children(send).Where(entry => {
            var links = KismetHelper.GetOutputLinksOfNode(entry);
            return links.Count == 1 && links[0].Count == 0;
        }).ToArray();
        if (terminals.Length != 1) throw new InvalidDataException("Email sending has no unambiguous insertion point.");
        var lastSend = terminals[0];
        var lastRead = readDonor;
        var readExit = Next(lastRead);
        var displayExit = Find(package, "Display_Messages.SeqCond_CompareBool_0");
        var incoming = KismetHelper.FindOutputConnectionsToNode(displayExit, Children(display));
        if (incoming.Count != 1) throw new InvalidDataException("Email display has no unambiguous insertion point.");
        var lastDisplay = incoming[0];
        var displayVariables = lastDisplay.GetProperty<ArrayProperty<StructProperty>>("VariableLinks")
            ?? throw new InvalidDataException("Email display lacks its shared GUI variables.");
        var archiveSwitch = Find(package, "Archive_Message.SeqAct_Switch_0");
        var archiveExit = Find(package, "Archive_Message.BioSeqAct_PMCheckConditional_1");
        var archiveLinks = KismetHelper.GetOutputLinksOfNode(archiveSwitch);
        if (archiveLinks.Count == 0 || archiveLinks[0].Count != 1 || archiveLinks[0][0].LinkedOp is not ExportEntry setDonor)
            throw new InvalidDataException("Email archive switch lacks its status assignment.");
        var statusDonor = Linked(setDonor, "Target");
        if (setDonor.ClassName != "SeqAct_SetInt" || statusDonor.ClassName != "BioSeqVar_StoryManagerInt")
            throw new InvalidDataException("Email archive switch has incompatible variables.");
        int count = archiveSwitch.GetProperty<IntProperty>("LinkCount")?.Value
            ?? throw new InvalidDataException("Email archive switch lacks its link count.");
        if (count != archiveLinks.Count) throw new InvalidDataException("Email archive switch link count is inconsistent.");
        int messageId = archiveLinks.Count + 1;
        for (int index = 0; index < emails.Length; index++)
        {
            cancellation.ThrowIfCancellationRequested();
            var email = emails[index];
            string name = $"Deployd_Email_{index}";
            var newSend = Clone(email.InMemoryBool.HasValue ? boolDonor : sendDonor, send, name);
            Plot(One(newSend, "BioSeqAct_PMCheckConditional"), email.Conditional);
            Plot(One(newSend, "BioSeqAct_PMExecuteTransition"), email.Transition);
            if (email.InMemoryBool is int installed) Plot(One(newSend, "BioSeqAct_PMCheckState"), installed);
            Connect(lastSend, newSend);
            lastSend = newSend;

            var newRead = Clone(readDonor, read, name);
            Plot(One(newRead, "BioSeqVar_StoryManagerInt"), email.Status);
            if (email.ReadTransition is int transition)
            {
                var added = EntryCloner.CloneEntry(One(sendDonor, "BioSeqAct_PMExecuteTransition"));
                KismetHelper.AddObjectToSequence(added, newRead);
                Plot(added, transition);
                Connect(One(newRead, "SeqAct_SetInt"), added);
                Connect(added, One(newRead, "SeqAct_FinishSequence"));
            }
            Connect(lastRead, newRead);
            lastRead = newRead;

            var newDisplay = Clone(displayDonor, display, name);
            newDisplay.WriteProperty(displayVariables);
            Plot(One(newDisplay, "BioSeqVar_StoryManagerInt"), email.Status);
            var choice = One(newDisplay, "BioSeqAct_AddChoiceGUIElement");
            StringRef(Linked(choice, "ChoiceName"), email.Title);
            StringRef(Linked(choice, "ChoiceDescription"), email.Description);
            var choiceId = Linked(choice, "ChoiceID");
            if (choiceId.ClassName != "SeqVar_Int") throw new InvalidDataException("Email display ID is not an integer.");
            choiceId.WriteProperty(new IntProperty(messageId, "IntValue"));
            var variables = KismetHelper.GetVariableLinksOfNode(choice);
            foreach (var link in variables.Where(link => link.LinkDesc == "oChoiceImage")) link.LinkedNodes.Clear();
            KismetHelper.WriteVariableLinksToNode(choice, variables);
            // The shipped Cerberus message is disabled; new messages must enter its status comparison.
            Connect(One(newDisplay, "SeqEvent_SequenceActivated"), One(newDisplay, "SeqCond_CompareInt"));
            Connect(lastDisplay, newDisplay);
            lastDisplay = newDisplay;

            var set = EntryCloner.CloneEntry(setDonor);
            KismetHelper.AddObjectToSequence(set, archive);
            var status = EntryCloner.CloneEntry(statusDonor);
            KismetHelper.AddObjectToSequence(status, archive);
            Plot(status, email.Status);
            var setVariables = KismetHelper.GetVariableLinksOfNode(set);
            foreach (var link in setVariables.Where(link => link.LinkDesc == "Target")) link.LinkedNodes = new List<IEntry> { status };
            KismetHelper.WriteVariableLinksToNode(set, setVariables);
            Connect(set, archiveExit);
            KismetHelper.CreateNewOutputLink(archiveSwitch, $"Link {messageId - 1}", set);
            messageId++;
            count++;
        }
        Connect(lastRead, readExit);
        Connect(lastDisplay, displayExit);
        archiveSwitch.WriteProperty(new IntProperty(count, "LinkCount"));
    }
}
