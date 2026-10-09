#!/usr/bin/env python3
"""Opt-in real DeepSeek + Codex, isolated relay/members, actual PTY keystrokes.
Requires pyte for saved screen captures. Never sends to the user's real team.
"""
import codecs, fcntl, hashlib, json, os, pathlib, pty, select, sqlite3, struct, subprocess, sys, termios, time, uuid
import pyte
from check_task_workflow_real import config_text, read
REPO = pathlib.Path(__file__).resolve().parents[1]
BIN = REPO / 'target/debug/xxassxx'
ROOT = REPO / 'smoke-output' / ('issues-1-2-' + str(uuid.uuid4())[:8])
ROOT.mkdir(parents=True, mode=0o700)
ENV = {**os.environ, 'TERM':'xterm-256color', 'ISSUES_A_TOKEN':'isolated-a-'+str(uuid.uuid4()), 'ISSUES_B_TOKEN':'isolated-b-'+str(uuid.uuid4())}
REPORT = {'kind':'real models, two isolated clients on one Mac and loopback relay; no cross-machine test', 'binary_sha256':hashlib.sha256(BIN.read_bytes()).hexdigest(), 'checks':[], 'status':'running'}
def save(): (ROOT/'evidence.json').write_text(json.dumps(REPORT,ensure_ascii=False,indent=2))
def check(label, **facts):
    REPORT['checks'].append({'check':label,**facts}); save(); print(label,flush=True)
def run(m,*args):
    p=subprocess.run([str(BIN),'--db',str(ROOT/m/'member.sqlite3'),*map(str,args)],env=ENV,capture_output=True,text=True,timeout=150)
    if p.returncode: raise RuntimeError(' '.join(map(str,args[:2]))+': '+p.stderr[:1000])
    return json.loads(p.stdout)
class Terminal:
    def __init__(self,m):
        self.decoder=codecs.getincrementaldecoder("utf-8")(); self.m=m; self.master,self.slave=pty.openpty(); self.raw=bytearray()
        fcntl.ioctl(self.slave,termios.TIOCSWINSZ,struct.pack('HHHH',44,160,0,0))
        self.screen=pyte.Screen(160,44); self.stream=pyte.Stream(self.screen)
        self.proc=subprocess.Popen([str(BIN),'open',str(ROOT/m/'work'),'--directory',str(ROOT/m)],stdin=self.slave,stdout=self.slave,stderr=self.slave,env=ENV,start_new_session=True)
        self.drain(1)
    def drain(self,seconds=.2):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if select.select([self.master],[],[],.03)[0]:
                try: data=os.read(self.master,65536)
                except OSError: break
                self.raw.extend(data); self.stream.feed(self.decoder.decode(data))
    def keys(self,keys): os.write(self.master,keys); self.drain()
    def text(self,text): self.keys(b'\x1b[200~'+text.encode()+b'\x1b[201~'); self.keys(b'\r')
    def capture(self,label):
        self.drain(.6); text='\n'.join(self.screen.display); (ROOT/(self.m+'-'+label+'.txt')).write_text(text)
        (ROOT/('terminal-'+self.m+'.ansi')).write_bytes(self.raw)
        print('SCREEN '+self.m+' '+label+'\n'+text,flush=True); return text
    def close(self):
        self.keys(b'\x11'); self.proc.wait(timeout=10); (ROOT/('terminal-'+self.m+'.ansi')).write_bytes(self.raw)
        os.close(self.master); os.close(self.slave)
uis=[]; relay=None
try:
    effective=json.loads(read(pathlib.Path.home()/'.local/share/xxassxx/client/member.sqlite3','SELECT config FROM identity')[0]['config'])
    cfg=ROOT/'relay.toml'; cfg.write_text('team_id="issues-test"\n'+''.join('[[members]]\nmember_id="'+m+'"\ndisplay_name="'+m+'"\ncredential_env="ISSUES_'+m.upper()+'_TOKEN"\n' for m in ['a','b']))
    relay=subprocess.Popen([str(BIN),'--db',str(ROOT/'relay.sqlite3'),'mailbox','serve','--listen','127.0.0.1:0','--config',str(cfg)],stdout=subprocess.PIPE,stderr=open(ROOT/'relay.log','w'),text=True,env=ENV)
    url='http://'+json.loads(relay.stdout.readline())['listening']
    for m in ['a','b']:
        work=ROOT/m/'work'; work.mkdir(parents=True)
        run(m,'init'); run(m,'member','init','--id',m,'--name',m,'--team','issues-test')
        model=effective['model']; model['thinking']=False; model['timeout_secs']=120
        c={'mailbox_url':url,'credential_env':'ISSUES_'+m.upper()+'_TOKEN','contacts':[{'member_id':p,'display_name':p} for p in ['a','b'] if p!=m], 'executor':{'mode':'auto','codex':effective['executor']['codex'],'workdir':str(work),'timeout_secs':240},'model':model}
        f=ROOT/m/'member.toml'; f.write_text(config_text(c)); run(m,'member','configure',f); run(m,'roots','add',work)
    payload=b'time,temperature\n0,21.5\n1,23.0\n'
    (ROOT/'b/work/temperature.csv').write_bytes(payload)
    run('a','files','preferences','--auto-receive','0')
    a=Terminal('a'); b=Terminal('b'); uis=[a,b]
    def wait(predicate,label,timeout=300):
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            for ui in uis: ui.drain(.1)
            result=predicate()
            if result:return result
        for ui in uis:ui.capture('timeout')
        raise RuntimeError('timeout: '+label)
    a.text('请只回复“终端检查正常”。这是普通对话，不要建立任务。')
    a.capture('local-progress')
    wait(lambda: read(ROOT/'a/member.sqlite3',"SELECT body FROM app_messages WHERE sender='butler' AND body LIKE '%终端检查正常%'"),'local answer')
    wait(lambda: read(ROOT/'a/member.sqlite3',"SELECT id FROM app_commands WHERE state='done'"),'local command finished')
    screen=a.capture('local-final'); assert '当前进展' not in screen
    check('local chat through PTY completes; progress removed after final answer')
    a.text('@b 请把你工作目录中现有的 temperature.csv 原文件作为实际附件发给我。只发送这一个文件，不分析、不运行脚本，不需要生成新文件。')
    row=wait(lambda:read(ROOT/'a/member.sqlite3',"SELECT id FROM app_tasks WHERE state='draft' AND json_array_length(json_extract(draft,'$.deliverables'))>0"),'draft')[0]
    task=row['id']; REPORT['task_id']=task; save()
    a.capture('draft'); a.keys(b'\x13')
    wait(lambda:run('a','app','task',task)['state']!='draft','confirmed')
    a.capture('peer-progress')
    def pending():
        t=run('a','app','task',task)
        if t['state']=='needs_attention': raise RuntimeError(t.get('error') or t.get('waiting_for'))
        return t if t.get('delivery',{}).get('items') else None
    t=wait(pending,'sender permission and review')
    a.capture('sender-authorization'); assert t['delivery']['next_owner']=='b',t['delivery']
    assert run('a','files','list')==[]
    a.keys(b'\x06'); screen=a.capture('attachments-before-authorization'); assert '发送方在自己的终端' in screen
    check('recipient has no download before sender authorization; task and CtrlF identify sender',delivery=t['delivery'])
    a.close(); a=Terminal('a'); uis[0]=a
    a.keys(b'\x14'); a.keys(b'\r')
    screen=a.capture('reopened-wait'); assert '等待 @b 允许发送附件' in screen and '已等待' in screen
    check('progress and responsible member survive closing and reopening the TUI')
    relay.terminate(); relay.wait(timeout=10)
    wait(lambda:read(ROOT/'a/member.sqlite3',"SELECT state FROM app_network WHERE state='unreachable'"),'connection loss',30)
    screen=a.capture('connection-lost'); assert '信箱连接中断' in screen
    relay=subprocess.Popen([str(BIN),'--db',str(ROOT/'relay.sqlite3'),'mailbox','serve','--listen',url.removeprefix('http://'),'--config',str(cfg)],stdout=subprocess.PIPE,stderr=open(ROOT/'relay-restarted.log','w'),text=True,env=ENV)
    json.loads(relay.stdout.readline())
    wait(lambda:read(ROOT/'a/member.sqlite3',"SELECT state FROM app_network WHERE state='connected'"),'reconnected',40)
    check('PTY distinguishes connection loss and daemon reconnects without replaying execution')
    a.keys(b'\x1b'); a.text('我按 CtrlF 没有可下载文件。现在具体该谁做什么？不要新建或重跑任务。')
    wait(lambda:read(ROOT/'a/member.sqlite3',"SELECT id FROM app_commands WHERE body LIKE '我按 CtrlF%' AND state NOT IN ('pending','processing')"),'follow-up answer')
    a.capture('followup'); check('real model answers follow-up using transport context')
    b.keys(b'\x06'); b.keys(b'\r'); b.capture('sender-manifest'); b.keys(b'a')
    batch=wait(lambda:next((v for v in run('a','files','list') if v['state']=='offered'),None),'actionable receive entry')
    a.keys(b'\x06'); a.keys(b'\r'); a.capture('download-ready'); a.keys(b'd')
    t=wait(lambda:(lambda t: t if t['state']=='completed' else None)(run('a','app','task',task)),'final delivery')
    a.keys(b'\x1b'); a.keys(b'\x1b'); screen=a.capture('delivery-final')
    assert '已收到并校验' in screen and '当前进展' not in screen
    received=run('a','files','show',batch['id']); path=pathlib.Path(received['local_files'][0]['path'])
    assert path.read_bytes()==payload
    runs=read(ROOT/'b/member.sqlite3','SELECT state,turn_completed,exit_code FROM runs')
    assert len(runs)==1 and runs[0]['turn_completed']==1 and runs[0]['exit_code']==0,runs
    check('sender PTY authorizes; recipient PTY downloads; meaningful final answer and exact bytes',sha256=hashlib.sha256(payload).hexdigest(),runs=runs,final=t['result']['body'])
    REPORT['status']='passed'; save(); print('EVIDENCE '+str(ROOT/'evidence.json'),flush=True)
except Exception as e:
    REPORT['status']='failed'; REPORT['error']=str(e); save()
    for ui in uis:
        try:ui.capture('failure')
        except Exception:pass
    raise
finally:
    for ui in uis:
        try:ui.close()
        except Exception:ui.proc.terminate()
    for m in ['a','b']:
        if (ROOT/m/'member.sqlite3').exists():
            try:run(m,'service','stop','--wait-secs','30')
            except Exception:pass
    if relay:relay.terminate(); relay.wait(timeout=10)
