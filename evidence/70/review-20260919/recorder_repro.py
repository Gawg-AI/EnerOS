import argparse, importlib.util, json, tempfile
from pathlib import Path
from unittest.mock import patch
source=Path(__file__).resolve().parents[3]/'tools/run70-candidate-evidence.py'
spec=importlib.util.spec_from_file_location('recorder70',source); m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
with tempfile.TemporaryDirectory(prefix='review70-recorder-') as td:
    m.ROOT=Path(td);m.EVID_DIR=m.ROOT/'evidence/70/70.15'
    (m.ROOT/'artifact.raw').write_bytes(b'test artifact')
    def lin(profile,artifact):
        return dict(source_commit='a'*40,dirty_tree_digest='b'*64,toolchain_digest='c'*64,platform_inventory_digest='d'*64,artifact_sha256=m.sha256_file(artifact) if artifact else '')
    def emit(cmd,**kw):
        p=m.ROOT/cmd[cmd.index('--out')+1];p.parent.mkdir(parents=True,exist_ok=True)
        p.write_text(json.dumps({'verdict':cmd[cmd.index('--verdict')+1]}))
        return argparse.Namespace(returncode=0,stdout='',stderr='')
    for label,log,rc in [('nonzero-exit','VOL70.1-GATE: PASS\n',1),('wrong-gate','VOL70.2-GATE: PASS\n',0)]:
        (m.ROOT/'run.log').write_text(log)
        a=argparse.Namespace(log='run.log',artifact='artifact.raw',profile='qemu-test',ran_artifact_sha_file='',verdict_line='',case='CORE-70-R01',gate='VOL70.1-GATE',command='real gate command',exit_code=rc,scope='L2',oracle='external test',positive='ok:PASS:ok',negative='bad:FAIL:rejected',artifact_name='test')
        with patch.object(m,'lineage',lin),patch.object(m.subprocess,'run',emit):
            got=m.cmd_record(a)
        rec=next(m.EVID_DIR.glob('*/CORE-70-R01.run-record.json'))
        print('BUG REPRO',label,'expected=reject','actual_return=',got,'actual_verdict=',json.loads(rec.read_text())['verdict'])
