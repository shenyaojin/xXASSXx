#!/usr/bin/env python3
"""Opt-in real acceptance: two local Rust clients + mailbox; no fake business results."""
import datetime, json, pathlib, sqlite3, subprocess, time
from phase5_demo import cli,session,submit,BIN

def read(root,m,sql,args=()):
    with sqlite3.connect(root/m/'member.sqlite3') as c:
        c.row_factory=sqlite3.Row;return [dict(r) for r in c.execute(sql,args)]
def wait(fn,condition,what,timeout=240):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        value=fn()
        if condition(value):return value
        time.sleep(.4)
    raise RuntimeError(f'Timeout waiting for {what}: {value}')
def command(root,m,i):
    v=wait(lambda:cli(root,m,'app','command',i['request_id']),lambda c:c['state'] not in ['pending','processing'],'owner command')
    if v['state']!='done':raise RuntimeError('Owner command failed: '+json.dumps(v,ensure_ascii=False))
    return v
def snapshot(root,m,sid=None):return cli(root,m,'app','snapshot',sid or session(root,m))
def card(root,m,id):return next(t for t in snapshot(root,m)['tasks'] if t['id']==id)
def task_state(root,m,id,state):
    def state_fn():
        t=card(root,m,id)
        if t['state'] in ['failed','timed_out','needs_attention','limit_reached']:raise RuntimeError('Execution failed: '+json.dumps(t,ensure_ascii=False))
        return t
    return wait(state_fn,lambda t:t['state']==state,'task '+state,300)
def turn(root,m,body,**kw):
    i=submit(root,m,body,**kw);return i,command(root,m,i)
def peer_history(root,sid):
    failures=read(root,'test',"SELECT error FROM messages WHERE direction='in' AND state='failed'")
    if failures: raise RuntimeError('Peer request failed: '+json.dumps(failures))
    return snapshot(root,'owner',sid)['messages']
def smoke(root):
    root=pathlib.Path(root);evidence=root/'evidence';evidence.mkdir(exist_ok=True)
    report={'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'root':str(root),'kind':'REAL DeepSeek + official Codex, two isolated local clients','checks':[]}
    def passed(name,**v):report['checks'].append({'name':name,**v});(evidence/'real.json').write_text(json.dumps(report,ensure_ascii=False,indent=2));print('PASS '+name,flush=True)
    try:
        time.sleep(2)
        assert not read(root,'owner','SELECT * FROM app_model_runs') and not read(root,'test','SELECT * FROM model_runs')
        passed('opening environment does not call models or Codex')
        one,_=turn(root,'owner','这轮讨论的代号是银杏，只是讨论，不要建立任务。请记住它，并问我接下来想讨论什么。')
        two,_=turn(root,'owner','刚才我给这轮讨论起的代号是什么？请直接回答。')
        answers=[m for m in snapshot(root,'owner')['messages'] if m.get('command_id')==two['request_id'] and m['kind']=='answer']
        assert answers and '银杏' in answers[-1]['body'],answers
        passed('persistent natural follow-up',request_ids=[one['request_id'],two['request_id']],answer=answers[-1]['body'])
        msg,_=turn(root,'owner','请说明你有哪些已共享、可供协作的材料。只列材料元数据，不分析文件，不启动 Codex。',peer='test')
        peer_session=msg['session_id']
        history=wait(lambda:peer_history(root,peer_session),lambda a:any(m['sender']=='test' and m['kind']=='peer_message' for m in a),'actual peer reply')
        reply=[m for m in history if m['sender']=='test' and m['kind']=='peer_message'][-1]
        assert 'Synthetic' in reply['body'] or 'CSV' in reply['body'],reply
        passed('addressed test butler reply in same session',session=peer_session,reply=reply['body'])
        local,c=turn(root,'owner','请创建一个只读文件分析任务：读取我接下来会选择授权的 tiny.csv，计算 amount 列总和并给出文件来源。不要运行任何脚本。')
        assert c['task_id'],c
        tid=c['task_id'];assert card(root,'owner',tid)['state']=='awaiting_authorization'
        passed('natural language creates task waiting for explicit owner file selection',task=tid)
        turn(root,'owner','允许导入合成样本目录',action='root_add',payload={'path':str(root/'projects/owner')})
        auth,_=turn(root,'owner','仅授权 tiny.csv',action='authorize',task=tid,payload={'files':[str(root/'projects/owner/tiny.csv')]})
        completed=task_state(root,'owner',tid,'completed')
        assert completed['result']['sources'] and '9' in completed['result']['body'],completed
        passed('authorized local Codex file result',task=tid,result=completed['result'],artifact=completed['artifact'])
        # Exact request replay must not produce another run.
        before=len(read(root,'owner','SELECT * FROM workflow_runs'));cli(root,'owner','app','submit',data=auth);time.sleep(2);assert before==len(read(root,'owner','SELECT * FROM workflow_runs'))
        passed('duplicate authorization does not start Codex twice',runs=before)
        goal='只读已授权 CSV。先读取材料，然后必须向发起方询问一个尚未提供的调整系数；在获得系数前不要计算最终值。收到明确系数后，将 amount 总和乘以该系数，返回计算过程和来源。禁止运行脚本。'
        ri,rc=turn(root,'owner',goal,action='create_task',payload={'title':'对方样本与调整系数','peer':'test'})
        rid=rc['task_id'];assert card(root,'owner',rid)['state']=='waiting_peer_authorization'
        peer=wait(lambda:snapshot(root,'test')['tasks'],lambda ts:any(t['origin_request'] for t in ts),'peer owner authorization request')
        peer=next(t for t in peer if t['origin_request']);assert peer['state']=='awaiting_authorization'
        turn(root,'test','允许导入合成样本目录',action='root_add',payload={'path':str(root/'projects/test')})
        turn(root,'test','测试主人明确选定 tiny.csv',action='authorize',task=peer['id'],payload={'files':[str(root/'projects/test/tiny.csv')]})
        task_state(root,'owner',rid,'offer_available')
        turn(root,'owner','使用对方准备的具体材料',action='use_offer',task=rid)
        question=task_state(root,'owner',rid,'waiting_user')
        passed('remote owner authorizes and Codex asks real clarification',task=rid,question_id=question['question_id'])
        answer,ac=turn(root,'owner','调整系数采用 3。请将这个系数回复给对方；只补充系数，不扩大文件授权。',task=rid,channel='second-test')
        assert ac['state']=='done'
        final=task_state(root,'owner',rid,'completed');assert final['result']['sources'] and '27' in final['result']['body'],final
        runs=read(root,'test','SELECT r.id,r.session_id,r.resumed_session_id,r.state FROM runs r JOIN workflow_runs w ON w.run_id=r.id')
        assert len(runs)==2 and runs[0]['session_id']==runs[1]['session_id'] and runs[1]['resumed_session_id']==runs[0]['session_id'],runs
        passed('real remote result and exact ID resume through second channel',task=rid,result=final['result'],codex_runs=runs,answer_request=answer['request_id'])
        view=snapshot(root,'owner');assert view['presence']['members'][0]['presence']['state']=='recent'
        passed('server-timed presence without model calls',presence=view['presence'])
        for m in ['owner','test']:
            details={'identity':snapshot(root,m)['identity'],'owner_models':read(root,m,'SELECT id,command_id,state,calls,trace,error FROM app_model_runs'),'peer_models':read(root,m,'SELECT id,state,calls,error FROM model_runs'),'workflow_models':read(root,m,'SELECT id,state,calls,error FROM workflow_models'),'runs':read(root,m,'SELECT id,task_id,state,session_id,resumed_session_id,mcp_initialized,reads,submissions,turn_completed,exit_code,codex_version FROM runs'),'mcp_file_reads':read(root,m,'SELECT workflow_id,run_id,version_id,path,sha256,offset,bytes FROM workflow_file_reads')}
            (evidence/f'{m}.json').write_text(json.dumps(details,ensure_ascii=False,indent=2))
        report['status']='passed';report['completed_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat()
        import tomllib
        cfg=tomllib.loads((root/'owner/member.toml').read_text())
        report['model']={k:cfg['model'].get(k) for k in ['provider','model','thinking','max_model_calls','max_tool_rounds','max_tokens']}
        report['codex_version']=subprocess.check_output([cfg['executor']['codex'],'--version'],text=True).strip()
    except Exception as e:
        report['status']='failed';report['error']=str(e);raise
    finally:(evidence/'real.json').write_text(json.dumps(report,ensure_ascii=False,indent=2))
if __name__=='__main__':
    import sys;smoke(pathlib.Path(sys.argv[1]))
