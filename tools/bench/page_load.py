#!/usr/bin/env python3
"""Page-load A/B benchmark.

Usage: page_load.py OUT_DIR ROUNDS SHOT_A SHOT_B

Serves a local Speedometer 3.1 checkout (~/suites/speedometer-3.1-*/resources,
set SPEEDOMETER_RESOURCES to override) and loads every workload page listed in
resources/tests.mjs with two `shot` builds, alternating which goes first, on
CPU 0. Each load is a fresh process (`shot URL out.png 1280 900`), so the times
include engine start-up, fetching, parsing, scripts, style, layout and the first
paint. Records wall time and shot's phase timings in OUT_DIR/runs.json and prints
the totals.
"""
import sys,subprocess,time,json,os,re,random,http.server,threading,functools
out,rounds,A,B=sys.argv[1],int(sys.argv[2]),sys.argv[3],sys.argv[4]
root=os.environ.get('SPEEDOMETER_RESOURCES') or os.path.expanduser('~/suites/speedometer-3.1-1386415be8fe/resources')
urls=re.findall(r'url: "([^"]*)"',open(os.path.join(root,'tests.mjs')).read())
seen=[];[seen.append(u) for u in urls if u not in seen]
class Q(http.server.SimpleHTTPRequestHandler):
    def log_message(self,*a): pass
srv=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Q,directory=root)); port=srv.server_port
threading.Thread(target=srv.serve_forever,daemon=True).start()
os.makedirs(out,exist_ok=True); res=[]
def ms(s,key):
    m=re.search(key+r' ([\d.]+)(ms|µs|s)\b',s)
    if not m: return None
    v=float(m.group(1)); return v/1000 if m.group(2)=='µs' else (v*1000 if m.group(2)=='s' else v)
for r in range(rounds):
    for u in seen:
        order=[('A',A),('B',B)] if (r+len(res))%2==0 else [('B',B),('A',A)]
        for arm,b in order:
            t=time.monotonic()
            p=subprocess.run(['taskset','-c','0',b,f'http://127.0.0.1:{port}/{u}',os.path.join(out,'last.png'),'1280','900'],capture_output=True,text=True,timeout=120)
            wall=(time.monotonic()-t)*1000
            e=p.stderr+p.stdout
            res.append({'round':r,'url':u,'arm':arm,'wall_ms':wall,'build_page':ms(e,'build_page'),'fonts':ms(e,'fonts'),'layout':ms(e,'load\\+layout'),'paint':ms(e,'paint')})
json.dump(res,open(os.path.join(out,'runs.json'),'w'))
import statistics as st,collections
by=collections.defaultdict(dict)
for x in res: by[(x['round'],x['url'])][x['arm']]=x
for k in ['wall_ms','build_page','fonts','layout','paint']:
    a=[v['A'][k] for v in by.values() if v['A'][k] is not None and v['B'][k] is not None]
    b=[v['B'][k] for v in by.values() if v['A'][k] is not None and v['B'][k] is not None]
    if a: print(f"{k:10} A total {sum(a):9.0f}  B total {sum(b):9.0f}  ratio {sum(a)/sum(b):.3f}  pages faster {sum(x>y for x,y in zip(a,b))}/{len(a)}")
print('LOADDONE')
