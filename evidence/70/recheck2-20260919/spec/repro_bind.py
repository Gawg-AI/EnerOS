from pathlib import Path
import json,subprocess
ROOT=Path('/home/ahx/文档/EnerOS'); OUT=Path(__file__).parent
hid=lambda x:', '.join(f'{x:02x}' for _ in range(16))
def serial_plan(tag,n,skip,vals):
 p=hid(tag); lines=[f'[svc-control-dispatch] PLAN-COMPILED batches=2 intents={n} skipped={skip} manual=0 compiler=[11] batch=[33] plan=[{p}]',f'[svc-control-dispatch] PLAN-HELD plan=[{p}] batches=2 intents={n}']
 for i in range(n):
  dev,pt=[(1,10),(8,1009),(9,1014)][i%3]
  lines.append(f'[svc-control-dispatch] HELD-COMMAND intent=[{hid(tag+16+i)}] state=Applied attempts=1 readback=Some({vals[i]}) reason=- plan=[{p}] dev=0x{dev:x} pt={pt} val={vals[i]}')
 lines.append(f'[svc-control-dispatch] HELD-PLAN-DONE plan=[{p}] batches=2 applied={n} blocked=0 failed=0 state=Committed')
 return lines

def events(vals,burst=False):
 out=[]
 for i,v in enumerate(vals):
  addr=hex(0x200+i%3);ts=1000 if burst else 1000+(i//3)*7
  out += [{'event':'WRITE','func':'fc06','addr':addr,'value':v,'ts':ts}, {'event':'READ','func':'fc03','addr':addr,'value':v}]
 return out

def run(name,lines,ev):
 sp=OUT/(name+'.serial.txt');tp=OUT/(name+'.jsonl')
 sp.write_text('\n'.join(lines)+'\n');tp.write_text('\n'.join(json.dumps(x) for x in ev)+'\n')
 r=subprocess.run(['python3',str(ROOT/'tools/r10_intent_bind.py'),str(sp),str(tp)],text=True,capture_output=True)
 report=f'{name}: rc={r.returncode}\nstdout={r.stdout}stderr={r.stderr}'
 (OUT/(name+'.result.txt')).write_text(report);print(report)
 return r

# P and Q have disjoint complete IDs but identical addressed values; only Q's
# transaction set is present. No txn/timestamp association can identify P.
vals=[1]*12
run('same-value-Q-only',serial_plan(0x10,12,0,vals)+serial_plan(0x60,12,0,vals),events(vals))
# Per-resource assignments [1,1,2,2] legitimately compile to 2 intents and 2
# skips. Both distinct-value commands occur at the same instant here.
burst=run('skipped-burst',serial_plan(0x10,6,6,[1]*3+[2]*3),events([1]*3+[2]*3,True))
print('A5b predicate for skipped-burst: SK=6 > 0, BW=6 <= I=6 -> PASS despite every asset span=0')
