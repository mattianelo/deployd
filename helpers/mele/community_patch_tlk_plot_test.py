"""Exercise verified TLK and plot inputs only in disposable staging."""

import json
from pathlib import Path
import subprocess
import tempfile

from build_support import directory, verify
from community_patch_test import GAME, MOD, INPUTS, copy, identity, verify_corpus
from community_patch_scripts_test import SCRIPT_INPUTS
from frozen_tlk import decode
from tlk_plot_corpus import PACKAGES


TLK = (f"{MOD}/GAME1_EMBEDDED_TLK/CombinedTLKMergeData.m3za", 85371,
       "b9ea76366e24f33166871a059284974a32e0ef3fb1c7ad97b41d40ea3fa58619")
PMU = (f"{MOD}/DLC_MOD_LE1CP/CookedPCConsole/PlotManagerUpdate.pmu", 3017,
       "bf2276c59d680cafe1a4fe8741e62be69d91cfe54629abd13075355c267a524f")
BASES = ("Core.pcc", "Engine.pcc", "GFxUI.pcc", "PlotManagerMap.pcc", "SFXOnlineFoundation.pcc",
         "SFXGame.pcc", "SFXStrategicAI.pcc", "SFXGameContent_Powers.pcc")


def transform(command, stage, request, name, kind, count, success=True):
    path = stage / (name + '.json')
    path.write_text(json.dumps(request))
    result = subprocess.run([*command, 'transform-' + kind, str(path)], capture_output=True, text=True)
    output = Path(request['output_root'])
    messages = [json.loads(line) for line in result.stdout.splitlines()]
    progress = [{"protocol": 1, "type": "progress", "completed": index, "total": count} for index in range(1, count + 1)]
    if not success:
        if result.returncode == 0 or messages != progress[:-1] or any(output.iterdir()):
            raise ValueError("Late TLK/plot failure did not preserve empty output staging")
        return
    if result.returncode != 0:
        raise ValueError(f"{kind} transformation failed: {result.stderr.strip()}")
    targets = request['targets'] if kind == 'tlk' else [request['target']['current']]
    outputs = [identity(output, target['path']) for target in targets]
    if messages != progress + [{"protocol": 1, "type": "complete", "outputs": outputs}]:
        raise ValueError("TLK/plot progress or output identity mismatch")
    if sorted(str(path.relative_to(output)) for path in output.rglob('*') if path.is_file()) != sorted(target['path'] for target in targets):
        raise ValueError("Unexpected TLK/plot output files")
    print(f"{kind} {name}: {count} operations and {len(outputs)} verified outputs", flush=True)


def run(root, semantic_tests, kind):
    if kind not in {'tlk', 'plot'}:
        raise ValueError("Unknown TLK/plot corpus selection")
    names = [name for name in PACKAGES if name not in {'PlotManager.pcc', 'SFXStrategicAI.pcc', 'SFXGameContent_Powers.pcc'}]
    corpus = {'codec': INPUTS['codec']}
    if kind == 'tlk':
        corpus.update({name: PACKAGES[name] for name in names})
        corpus['tlk'] = TLK
    else:
        corpus.update({name: PACKAGES[name] if name in PACKAGES else SCRIPT_INPUTS[name] for name in (*BASES, 'PlotManager.pcc')})
        corpus['pmu'] = PMU
    sources = verify_corpus(root / 'docs/modTesting', corpus)
    configuration = json.loads((root / 'helpers/mele/toolchain.json').read_text())
    build = directory(Path('/build/mele'))
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / 'dotnet'),
               str(build / 'source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll')]
    try:
        with tempfile.TemporaryDirectory(dir=build, prefix='community-patch-' + kind + '-') as temporary:
            stage = Path(temporary)
            for location in ['input', 'original', 'merged', 'reapplied', 'failed', 'verification', 'removed', 'combined', 'combined-verification']:
                (stage / location).mkdir()
            copy(sources['codec'], stage / 'game/Binaries/Win64/oo2core_8_win64.dll')
            if kind == 'tlk':
                changes = decode(sources['tlk'])
                if len(changes) != 126 or sum(len(change['strings']) for change in changes) != 1244:
                    raise ValueError('Unexpected frozen TLK update count')
                if sorted({Path(change['target']).name for change in changes}) != sorted(names):
                    raise ValueError('Unexpected frozen TLK package list')
                for name in names:
                    copy(sources[name], stage / 'input/CookedPCConsole' / name)
                request = dict(protocol=1, operation='le1-tlk', game_root=str(stage / 'game'), input_root=str(stage / 'input'),
                               output_root=str(stage / 'merged'), targets=[identity(stage / 'input', 'CookedPCConsole/' + name) for name in names], changes=changes)
                count = len(changes)
            else:
                target = 'CookedPCConsole/PlotManager.pcc'
                for name in (*BASES, 'PlotManager.pcc'):
                    copy(sources[name], stage / 'input/CookedPCConsole' / name)
                copy(sources['PlotManager.pcc'], stage / 'original' / target)
                manifest = 'DLC/DLC_MOD_LE1CP/CookedPCConsole/PlotManagerUpdate.pmu'
                copy(sources['pmu'], stage / 'input' / manifest)
                request = dict(protocol=1, operation='le1-plot', game_root=str(stage / 'game'), original_root=str(stage / 'original'),
                               input_root=str(stage / 'input'), output_root=str(stage / 'merged'),
                               target=dict(original=identity(stage / 'original', target), current=identity(stage / 'input', target)),
                               dependencies=[identity(stage / 'input', 'CookedPCConsole/' + name) for name in BASES],
                               contributions=[dict(dlc='DLC_MOD_LE1CP', mount=5, manifest=identity(stage / 'input', manifest))])
                count = 9
            immutable = {path: identity(path.parent, path.name) for path in stage.rglob('*') if path.is_file()}
            transform(command, stage, request, 'merged', kind, count)
            (stage / 'semantic-request.json').write_text(json.dumps(dict(request, output_root=str(stage / 'verification'))))
            if kind == 'tlk':
                repeated = dict(request, input_root=str(stage / 'merged'), output_root=str(stage / 'reapplied'),
                                targets=[identity(stage / 'merged', target['path']) for target in request['targets']])
                transform(command, stage, repeated, 'reapplied', kind, count)
                failed = dict(request, output_root=str(stage / 'failed'), changes=[*changes[:-1], dict(changes[-1], export='MissingExport')])
                transform(command, stage, failed, 'failed', kind, count, success=False)
            else:
                previous = stage / 'previous'
                for path in (stage / 'input').rglob('*'):
                    if path.is_file(): copy(path, previous / path.relative_to(stage / 'input'))
                copy(stage / 'merged' / target, previous / target)
                repeated = dict(request, input_root=str(previous), output_root=str(stage / 'reapplied'),
                                target=dict(request['target'], current=identity(previous, target)))
                transform(command, stage, repeated, 'reapplied', kind, count)
                removed = dict(repeated, output_root=str(stage / 'removed'), contributions=[], dependencies=[])
                transform(command, stage, removed, 'removed', kind, 0)
                if (stage / 'removed' / target).read_bytes() != sources['PlotManager.pcc'].read_bytes():
                    raise ValueError('Removing plot contributions did not restore original bytes')
                extra = 'DLC/DLC_MOD_TEST/CookedPCConsole/PlotManagerUpdate.pmu'
                text = ('public function bool F155(BioWorldInfo bioWorld, int Argument)\n{ return false; }\n'
                        'public function bool F1900000002(BioWorldInfo bioWorld, int Argument)\n{ return true; }\n'
                        'public function bool F1900000001(BioWorldInfo bioWorld, int Argument)\n{ return F1900000002(bioWorld, Argument); }\n')
                (stage / 'input' / extra).parent.mkdir(parents=True)
                (stage / 'input' / extra).write_text(text)
                combined = dict(request, output_root=str(stage / 'combined'), contributions=[dict(dlc='DLC_MOD_TEST', mount=10,
                                manifest=identity(stage / 'input', extra)), *request['contributions']])
                transform(command, stage, combined, 'combined', kind, count + 2)
                (stage / 'combined-request.json').write_text(json.dumps(dict(combined, output_root=str(stage / 'combined-verification'))))
                immutable[stage / 'input' / extra] = identity(stage / 'input', extra)
                bad = 'DLC/DLC_MOD_BAD/CookedPCConsole/PlotManagerUpdate.pmu'
                (stage / 'input' / bad).parent.mkdir(parents=True)
                (stage / 'input' / bad).write_text('public function bool F1900000003(BioWorldInfo bioWorld, int Argument)\n{ invalid syntax; }\n')
                failed = dict(request, output_root=str(stage / 'failed'), contributions=[*request['contributions'], dict(dlc='DLC_MOD_BAD', mount=20,
                              manifest=identity(stage / 'input', bad))])
                transform(command, stage, failed, 'failed', kind, count + 1, success=False)
                immutable.update({path: identity(path.parent, path.name) for path in previous.rglob('*') if path.is_file()})
            semantic_tests(root, corpus=stage, corpus_kind=kind)
            for path, spec in immutable.items(): verify(path, spec)
            print(f'{kind} reference, rebuilding, cancellation, failure, and input-integrity checks passed.', flush=True)
    finally:
        verify_corpus(root / 'docs/modTesting', corpus)
        print('Supplied game and mod files retain their pinned hashes.', flush=True)
