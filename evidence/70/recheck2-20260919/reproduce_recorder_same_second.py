import argparse, importlib.util, json, tempfile, sys
from datetime import datetime, timezone
class FrozenClock:
 @staticmethod
 def now(tz):return datetime(2026,9,19,6,0,0,tzinfo=timezone.utc)
serial=[0]
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('recorder70',Path('/home/ahx/文档/EnerOS/tools/run70-candidate-evidence.py'));m=importlib.util.module_from_spec(spec);sys.modules['recorder70']=m;spec.loader.exec_module(m)
with tempfile.TemporaryDirectory(prefix='recheck70-recorder-') as td:
 m.ROOT=Path(td);m.EVID_DIR=m.ROOT/'evidence/70/70.15'
 (m.ROOT/'artifact.raw').write_bytes(b'artifact')
 def lin(profile,artifact):return dict(source_commit='a'*40,dirty_tree_digest='b'*64,toolchain_digest='c'*64,platform_inventory_digest='d'*64,artifact_sha256=m.sha256_file(artifact) if artifact else '')
 def emit(cmd,**kw):
  serial[0]+=1; p=m.ROOT/cmd[cmd.index('--out')+1];p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps({'verdict':cmd[cmd.index('--verdict')+1], 'nonce':str(serial[0])}));return argparse.Namespace(returncode=0,stdout='',stderr='')
 a=argparse.Namespace(log='run.log',artifact='artifact.raw',profile='qemu-test',ran_artifact_sha_file='',verdict_line='',case='CORE-70-R01',gate='VOL70.1',command='gate',exit_code=0,scope='L2',oracle='external',positive='ok:PASS:ok',negative='bad:FAIL:rejected',artifact_name='test')
 with patch.object(m,'lineage',lin),patch.object(m.subprocess,'run',emit),patch.object(m,'datetime',FrozenClock):
  (m.ROOT/'run.log').write_text('VOL70.1-GATE: PASS\n');assert m.cmd_record(a)==0
  idx_path=next(m.EVID_DIR.glob('*/candidate-index.json'));idx_before=idx_path.read_bytes();entry=json.loads(idx_before)['cases'][0];env=m.ROOT/entry['envelope'];env_before=env.read_bytes()
  try:
   with patch.object(m,'save_index',side_effect=OSError('injected index write failure')):m.cmd_record(a)
  except OSError:pass
  else:raise AssertionError('fault not injected')
  print('index_unchanged=',idx_path.read_bytes()==idx_before)
  print('success_envelope_unchanged=',env.read_bytes()==env_before)
  print('index_envelope_digest_matches=',m.sha256_file(env)==entry['envelope_sha256'])
  assert env.read_bytes()==env_before,'Same-second retry and index failure overwrote previously indexed successful envelope'
