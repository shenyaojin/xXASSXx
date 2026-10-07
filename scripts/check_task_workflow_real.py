#!/usr/bin/env python3
"""Opt-in real model + official Codex. Two isolated local clients, synthetic files.
Uses application/TUI actions only; SQLite reads collect evidence, never fabricate state.
Existing production DB is opened read-only solely to obtain effective model configuration.
"""
import datetime, fcntl, json, os, pathlib, pty, select, sqlite3, struct, subprocess, sys, termios, time, uuid
from zoneinfo import ZoneInfo
REPO=pathlib.Path(__file__).resolve().parents[1]
BIN=REPO/'target/debug/xxassxx'

def read(db,sql,args=()):
    with sqlite3.connect('file:'+str(db)+'?mode=ro',uri=True) as c:
        c.row_factory=sqlite3.Row
        return [dict(r) for r in c.execute(sql,args)]
def toml_value(v):return json.dumps(v,ensure_ascii=False)
def config_text(cfg):
    out=[]
    for k,v in cfg.items():
        if v is not None and not isinstance(v,(dict,list)):out.append(k+' = '+toml_value(v))
    for section in ['executor','model']:
        out.append('['+section+']')
        for k,v in cfg[section].items():
            if v is not None:out.append(k+' = '+toml_value(v))
    for c in cfg['contacts']:
        out.append('[[contacts]]');out.extend(k+' = '+toml_value(v) for k,v in c.items())
    return '\n'.join(out)+'\n'
class Terminal:
    def __init__(self,root,env):
        self.master,self.slave=pty.openpty();fcntl.ioctl(self.slave,termios.TIOCSWINSZ,struct.pack('HHHH',38,150,0,0));self.data=bytearray()
        self.proc=subprocess.Popen([str(BIN),'open',str(root/'a/work'),'--directory',str(root/'a')],stdin=self.slave,stdout=self.slave,stderr=self.slave,env={**env,'TERM':'xterm-256color'},start_new_session=True)
        self.drain(1)
    def drain(self,seconds=.25):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if select.select([self.master],[],[],.03)[0]:
                try:self.data.extend(os.read(self.master,65536))
                except OSError:break
    def keys(self,data):os.write(self.master,data);self.drain()
    def text(self,text):self.keys(b'\x1b[200~'+text.encode()+b'\x1b[201~');self.keys(b'\r')
    def close(self,path):
        self.keys(b'\x11');self.proc.wait(timeout=10);path.write_bytes(self.data);os.close(self.master);os.close(self.slave)
def main():
    global BIN
    root=REPO/'smoke-output'/('task-v7-real-'+str(uuid.uuid4()));root.mkdir(parents=True);os.chmod(root,0o700)
    env={**os.environ,'TASK_TEST_A_TOKEN':'isolated-real-a-'+str(uuid.uuid4()),'TASK_TEST_B_TOKEN':'isolated-real-b-'+str(uuid.uuid4())}
    import hashlib,shutil
    frozen=root/'xxassxx'
    shutil.copy2(BIN,frozen);BIN=frozen
    binary_hash=hashlib.sha256(BIN.read_bytes()).hexdigest()
    report={'binary_sha256':binary_hash,'kind':'real DeepSeek and official Codex, two isolated clients on one Mac','root':str(root),'status':'running','user_actions':[],'checks':[]}
    evidence=root/'evidence.json'
    def save():evidence.write_text(json.dumps(report,ensure_ascii=False,indent=2))
    def check(name,**v):report['checks'].append({'name':name,**v});save();print(name,flush=True)
    def run(m,*args,data=None):
        cmd=[str(BIN),'--db',str(root/m/'member.sqlite3'),*map(str,args)]
        p=subprocess.run(cmd,input=None if data is None else json.dumps(data,ensure_ascii=False),text=True,capture_output=True,env=env,timeout=320)
        if p.returncode:raise RuntimeError(' '.join(map(str,args[:2]))+': '+p.stderr[-1500:])
        return json.loads(p.stdout)
    def wait(fn,predicate,label,timeout=600):
        end=time.monotonic()+timeout;last=None
        while time.monotonic()<end:
            v=fn()
            if predicate(v):return v
            state=v.get('state') if isinstance(v,dict) else None
            if state!=last:print(label+': '+str(state),flush=True);last=state
            if state in ['needs_attention','failed']:raise RuntimeError(label+': '+json.dumps(v,ensure_ascii=False)[-7000:])
            time.sleep(.6)
        raise RuntimeError('timeout: '+label)
    def submit(body,action='chat',task=None,payload=None):
        i={'request_id':str(uuid.uuid4()),'channel':'test-user-action','session_id':session,'task_id':task,'recipient':'a','body':body,'action':action,'payload':payload or {}}
        report['user_actions'].append(i);save();run('a','app','submit',data=i)
        return wait(lambda:run('a','app','command',i['request_id']),lambda v:v['state'] not in ['pending','processing'],'user command',150)
    relay=None;ui=None
    try:
        production=pathlib.Path.home()/'.local/share/xxassxx/client/member.sqlite3'
        effective=json.loads(read(production,'SELECT config FROM identity')[0]['config'])
        assert effective['model']['provider']=='deepseek'
        model=effective['model'];model['thinking']=False;model['timeout_secs']=120;model['max_tokens']=4096
        local=datetime.datetime.now(ZoneInfo('America/Denver'))
        relay_cfg=root/'relay.toml';relay_cfg.write_text('team_id="isolated-task-v7"\n[schedule]\nmeeting_interval_secs=30\nmeeting_wait_secs=5\nmeeting_max_rounds=2\ntimezone="America/Denver"\ndaily_time="'+local.strftime('%H:%M')+'"\n'+''.join('[[members]]\nmember_id="'+m+'"\ndisplay_name="'+m+'"\ncredential_env="TASK_TEST_'+m.upper()+'_TOKEN"\n' for m in ['a','b']))
        relay_log=open(root/'relay.log','w')
        relay=subprocess.Popen([str(BIN),'--db',str(root/'relay.sqlite3'),'mailbox','serve','--listen','127.0.0.1:0','--config',str(relay_cfg)],stdout=subprocess.PIPE,stderr=relay_log,text=True,env=env)
        url='http://'+json.loads(relay.stdout.readline())['listening']
        for m in ['a','b']:
            work=root/m/'work';work.mkdir(parents=True)
            run(m,'init');run(m,'member','init','--id',m,'--name',m,'--team','isolated-task-v7')
            cfg={'mailbox_url':url,'credential_env':'TASK_TEST_'+m.upper()+'_TOKEN','contacts':[{'member_id':p,'display_name':p} for p in ['a','b'] if p!=m],'executor':{'mode':'auto','codex':effective['executor']['codex'],'workdir':str(work),'timeout_secs':240},'model':model}
            file=root/m/'member.toml';file.write_text(config_text(cfg));run(m,'member','configure',file);run(m,'roots','add',work)
        (root/'b/work/direct.py').write_text('''"""Direct wave-response solver. CPU only. Supports irregular receiver geometry.
Memory estimate: 10 GiB at the target problem size. Runtime: about 18 minutes.
CLI: --mesh <json> --frequency <Hz> --tolerance <relative error>.
Recommended tolerance for engineering interpretation: 0.005.\n"""\nimport argparse\np=argparse.ArgumentParser()\np.add_argument('--mesh',required=True)\np.add_argument('--frequency',type=float,required=True)\np.add_argument('--tolerance',type=float,default=0.005)\n''')
        (root/'b/work/spectral.py').write_text('''"""Spectral wave-response solver. CPU only. Regular receiver geometry only.
Memory estimate: 3 GiB at the target problem size. Runtime: about 2 minutes.
CLI: --mesh <json> --frequency <Hz> --grid-spacing <m>.
Typical relative error: 0.02. Irregular geometry is not supported.\n"""\nimport argparse\np=argparse.ArgumentParser()\np.add_argument('--mesh',required=True)\np.add_argument('--frequency',type=float,required=True)\np.add_argument('--grid-spacing',type=float,required=True)\n''')
        (root/'b/work/README.txt').write_text('Synthetic experiment entry candidates. The workspace does not contain the customer receiver geometry, available memory, or experiment frequency; these are supplied by the requesting user. Do not execute these demonstration parsers to infer missing experimental inputs.\n')
        for m in ['a','b']:run(m,'service','start','--poll-secs','1','--reconnect')
        session=run('a','app','session','--project',root/'a/work')['session_id']
        submit('先记住项目背景：这次是工程解释，需要比较波场响应，目标相对误差不超过 1%。这是背景讨论，暂不委托任务。')
        c=submit('@b 请找适合这个实验目标的入口脚本，说明参数、候选差异、适用约束和选择依据。只读材料，不运行程序。先确认查找比较的目标；涉及候选适用性的具体现场条件，读到脚本后如有缺失再问我。')
        task=c['task_id'];view=lambda:run('a','app','task',task)
        draft=wait(view,lambda v:v['state']=='draft' and v['draft'].get('deliverables'),'draft')
        assert not read(root/'a/member.sqlite3','SELECT id FROM messages') and not read(root/'b/member.sqlite3','SELECT id FROM tasks')
        check('real intent restatement; no dispatch or Codex before confirmation',task_id=task,draft=draft['draft'])
        ui=Terminal(root,env);ui.keys(b'\x14');ui.keys(b'\r');ui.keys(b'\x13')
        report['user_actions'].append({'action':'TUI CtrlT Enter CtrlS','task_id':task,'revision':1});save()
        wait(view,lambda v:v['state']!='draft','intent confirmation',60)
        ui.close(root/'confirm.ansi');ui=None
        check('real TUI confirms; closes while both isolated daemons remain alive',background=[run(m,'service','status')['alive'] for m in ['a','b']])
        answered=False;answered_questions=set();deadline=time.monotonic()+600
        while time.monotonic()<deadline:
            v=view()
            failures=read(root/'b/member.sqlite3',"SELECT id,error FROM app_tasks WHERE state='needs_attention'")
            if failures:raise RuntimeError('B needs attention: '+json.dumps(failures,ensure_ascii=False))
            if v['state']=='needs_attention':raise RuntimeError(json.dumps(v,ensure_ascii=False)[-5000:])
            pending_questions=[q for q in v['questions'] if q['state']=='user' and q['revision']==v['revision']]
            if v['state']=='waiting' and pending_questions and pending_questions[0]['id'] not in answered_questions:
                check('real necessary question reached user',questions=v['questions'])
                answered_questions.add(pending_questions[0]['id'])
                ui=Terminal(root,env);ui.keys(b'\x14');ui.keys(b'\r');ui.keys(b'\x02');ui.keys(b'\r')
                ui.text('现场接收点是不规则布置，机器可用内存为 16 GiB，实验频率为 12 Hz；比较位移幅值，参考为直接法高精度解，误差用接收点集合的相对 L2 范数且目标 1%以内。语言与框架不限。现阶段只需依据现有合成材料推荐并说明参数，缺失网格数据与完整求解实现请列为限制，不要求实际运行或证明精度。')
                report['user_actions'].append({'action':'TUI answer selected question','task_id':task,'question_id':pending_questions[0]['id'],'answer':'irregular receivers,16 GiB,12 Hz,displacement amplitude,high-accuracy direct reference,relative receiver L2 error <=1%; documentation-based recommendation, list missing implementation/data; read only'})
                ui.close(root/'answer.ansi');ui=None;answered=True;time.sleep(1)
            if v['state']=='completed':break
            time.sleep(.8)
        else:raise RuntimeError('real task did not finish')
        assert answered,'Required user clarification was not exercised; do not claim full validation'
        runs=read(root/'b/member.sqlite3','SELECT id,task_id,state,session_id,resumed_session_id,mcp_initialized,reads,submissions,turn_completed,exit_code,codex_version FROM runs ORDER BY rowid')
        assert any(r['resumed_session_id'] for r in runs),'No actual resume observed'
        assert all(r['state']=='succeeded' and r['turn_completed'] and r['exit_code']==0 for r in runs)
        check('real Codex question, resume, final review and automatic archive',task_id=task,final=v['result'],runs=runs)
        before=len(read(root/'a/member.sqlite3',"SELECT id FROM app_messages WHERE kind='task_completed'"));assert before==1
        ui=Terminal(root,env);ui.keys(b'\x14');ui.keys(b'\r');ui.keys(b'\x04');ui.keys(b'\x1b');ui.keys(b'\x14');ui.keys(b'\r');ui.keys(b'\x05');ui.text('重开：保留前述实验条件，补充说明 direct.py 的 tolerance 与误差目标的关系。');ui.close(root/'reopen.ansi');ui=None
        reopened=wait(view,lambda v:v['revision']==2 and v['state']=='draft' and v['draft'].get('deliverables'),'reopened draft',180)
        check('actual TUI result/history and reopen preserve task ID',task_id=task,state=reopened['state'],revision=reopened['revision'])
        for m in ['a','b']:
            db=root/m/'member.sqlite3';data={}
            for table in ['task_revisions','task_journal','task_questions','task_executions','task_snapshots','task_system_events']:
                data[table]=read(db,'SELECT * FROM '+table)
            data['runs']=read(db,'SELECT id,task_id,state,session_id,resumed_session_id,workdir,reads,submissions,turn_completed,exit_code FROM runs')
            data['native_events']=read(db,"SELECT run_id,event FROM events WHERE json_extract(event,'$.item.type') IN ('command_execution','mcp_tool_call')")
            (root/(m+'-evidence.json')).write_text(json.dumps(data,ensure_ascii=False,indent=2))
        report['schedule']=read(root/'relay.sqlite3','SELECT kind,count(*) AS count,count(DISTINCT id) AS unique_ids FROM relay_system_events GROUP BY kind')
        report['meetings']=read(root/'relay.sqlite3','SELECT id,revision,state,round,merged_count FROM relay_meetings')
        report['daily']=read(root/'relay.sqlite3','SELECT * FROM relay_daily')
        report['status']='passed';save();print('REAL EVIDENCE '+str(evidence),flush=True)
    except Exception as e:
        report['status']='failed';report['error']=str(e);save();print('REAL FAILED '+str(evidence)+' '+str(e)[:1500],flush=True);raise
    finally:
        if ui:
            try:ui.close(root/'interrupted.ansi')
            except Exception:ui.proc.terminate()
        for m in ['a','b']:
            if (root/m/'member.sqlite3').exists():
                try:run(m,'service','stop','--wait-secs','30')
                except Exception:pass
        if relay:relay.terminate();relay.wait(timeout=10)
if __name__=='__main__':main()
