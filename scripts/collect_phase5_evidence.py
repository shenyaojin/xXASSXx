#!/usr/bin/env python3
"""Read-only export of allowlisted synthetic acceptance evidence; never copies DB/config/secrets."""
import datetime,hashlib,json,pathlib,sqlite3,sys,tomllib

def collect(root,out):
    out.mkdir(parents=True,exist_ok=True)
    report=json.loads((root/'evidence/real.json').read_text())
    cfg=tomllib.loads((root/'owner/member.toml').read_text())
    report['model']={k:cfg['model'].get(k) for k in ['provider','model','thinking','max_model_calls','max_tool_rounds','max_tokens']}
    (out/'real.json').write_text(json.dumps(report,ensure_ascii=False,indent=2))
    for member in ['owner','test']:
        with sqlite3.connect(root/member/'member.sqlite3') as c:
            c.row_factory=sqlite3.Row
            def rows(sql):return [dict(r) for r in c.execute(sql)]
            v={'collected_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'member':member,
               'owner_models':rows('SELECT id,command_id,state,calls,trace,error FROM app_model_runs'),
               'peer_models':rows('SELECT id,state,calls,error FROM model_runs'),
               'workflow_models':rows('SELECT id,state,calls,error FROM workflow_models'),
               'runs':rows('SELECT id,task_id,state,session_id,resumed_session_id,mcp_initialized,reads,submissions,turn_completed,exit_code,codex_version FROM runs'),
               'mcp_file_reads':rows('SELECT workflow_id,run_id,version_id,path,sha256,offset,bytes FROM workflow_file_reads'),
               'structured_tool_events':[]}
            for row in c.execute('SELECT run_id,event FROM events'):
                event=json.loads(row['event']);item=event.get('item',{})
                if item.get('type')=='mcp_tool_call':
                    v['structured_tool_events'].append({'run_id':row['run_id'],'type':event['type'],'server':item.get('server'),'tool':item.get('tool'),'status':item.get('status'),'saved_event_sha256':hashlib.sha256(row['event'].encode()).hexdigest()})
            for run in v['runs']:
                assert run['state']=='succeeded' and run['mcp_initialized']==1 and run['reads']>=1 and run['submissions']==1 and run['turn_completed']==1 and run['exit_code']==0,run
            (out/f'{member}.json').write_text(json.dumps(v,ensure_ascii=False,indent=2))
    print(json.dumps({'evidence':str(out),'real_status':report['status'],'credential_material_exported':False}))
if __name__=='__main__':collect(pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]))
