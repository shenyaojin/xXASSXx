#!/usr/bin/env python3
"""Deterministic Codex simulator. Real Rust MCP; no claim of real model reasoning."""
import json, os, re, subprocess, sys, tomllib, uuid
args=sys.argv[1:]
if args==['--version']: print('codex-cli simulated-business-task');sys.exit(0)
if args==['login','status']: print('Logged in using ChatGPT');sys.exit(0)
cfg={}
for n,arg in enumerate(args):
    if arg=='-c':
        key,value=args[n+1].split('=',1);cfg[key]=tomllib.loads('x='+value)['x']
profile=cfg['default_permissions']
assert profile in ['xxassxx_read','xxassxx_execute']
assert cfg[f'permissions.{profile}.network.enabled'] is False
assert cfg['features.shell_tool'] is True
prompt=sys.stdin.read();task=re.search(r'Assigned task_id: ([a-f0-9-]+)',prompt)[1];run=re.search(r'Current run_id: ([a-f0-9-]+)',prompt)[1]
session=str(uuid.uuid5(uuid.NAMESPACE_DNS,task))
if 'resume' in args: assert args[args.index('resume')+1]==session

def emit(v):print(json.dumps(v),flush=True)
emit({'type':'thread.started','thread_id':session});emit({'type':'turn.started'})
mcp=subprocess.Popen([cfg['mcp_servers.xxassxx.command'],*cfg['mcp_servers.xxassxx.args']],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
counter=0

def rpc(method,params={},notify=False):
    global counter
    counter+=1;v={'jsonrpc':'2.0','method':method,'params':params}
    if not notify:v['id']=counter
    mcp.stdin.write(json.dumps(v)+'\n');mcp.stdin.flush()
    if not notify:return json.loads(mcp.stdout.readline())['result']

def tool(name,params):
    v=rpc('tools/call',{'name':name,'arguments':params});assert not v.get('isError'),v
    emit({'type':'item.completed','item':{'type':'mcp_tool_call','server':'xxassxx','tool':name,'status':'completed','result':v}})
    return v['structuredContent']
rpc('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'simulated-business','version':'1'}});rpc('notifications/initialized',notify=True)
ctx=json.loads(tool('get_task',{'task_id':task})['input']);business=ctx['business_task'];base={'task_id':task,'run_id':run,'idempotency_key':run}
root=ctx['authorized_roots'][0]['path'];assert os.getcwd()==ctx['working_directory']
refresh='补充材料回归' in business['original']
if business['phase']=='execute':
    from pathlib import Path
    assert profile=='xxassxx_execute'
    access=ctx['execution_access'];workspace=Path(access['directory'])
    assert workspace==Path.cwd()
    permissions=cfg[f'permissions.{profile}.filesystem']
    assert permissions[str(workspace)]=='write'
    assert all(permissions[r]=='read' for r in access['read_roots'])
    source=Path(access['read_roots'][0])/'fast.py'
    assert source.read_text()=='iterations=10 # fast original'
    script=workspace/'calculation.py';script.write_text(source.read_text()+'\nprint(sum(range(iterations+1)))\n')
    if '撤销执行测试' in business['original']:
        subprocess.run([sys.executable,'-c',"import time,pathlib; time.sleep(20); pathlib.Path('late.txt').write_text('should not happen')"],check=True)
    run_result=subprocess.run([sys.executable,str(script)],capture_output=True,text=True,check=True)
    (workspace/'result.txt').write_text(run_result.stdout)
    (workspace/'stdout.log').write_text(run_result.stdout)
    emit({'type':'item.completed','item':{'type':'command_execution','command':'python calculation.py','exit_code':run_result.returncode,'status':'completed','aggregated_output':run_result.stdout}})
    tool('submit_task_result',{**base,'result':json.dumps({'body':'已在独立目录写入并实际运行，结果为55。原文件未修改。','files':[{'root':0,'path':p,'reason':'实际生成的结果证据'} for p in ['calculation.py','result.txt','stdout.log']]},ensure_ascii=False)})
elif business['phase']=='discover':
    paths=[]
    names=['fast.py','precise.py']
    if refresh and not any(q['state']=='investigating' for q in business['questions_and_answers']):names=['fast.py']
    for n,r in enumerate(ctx['authorized_roots']):
        for name in names:
            if os.path.isfile(os.path.join(r['path'],name)):paths.append({'root':n,'path':name,'reason':'candidate entry script'})
    tool('submit_task_result',{**base,'result':json.dumps({'body':'候选路径发现，尚未做固定版本分析。','files':paths})})
elif refresh:
    assert business['previous_execution']['phase']=='discover'
    assert business['previous_execution']['result']['body'].startswith('候选路径发现')
    if not any(f['path']=='precise.py' for f in business['material_sources']):
        tool('submit_task_question',{**base,'body':'请调查并补充原目录中的 precise.py；当前快照只有 fast.py','reason':'缺少比较所需的文件，不能从旧快照读取实时目录','known':'保留已读快照，需要新的发现和冻结'})
    else:
        texts=[];refs=[]
        for f in business['material_sources']:
            texts.append(open(os.path.join(root,f['snapshot_path'])).read());refs.append({'root':0,'path':f['snapshot_path'],'reason':'读取补齐后的新快照'})
        tool('submit_task_result',{**base,'result':json.dumps({'body':'补齐材料：'+' | '.join(texts),'files':refs},ensure_ascii=False)})
elif '跨端调查回归' in business['original']:
    if not any(q.get('answer') for q in business['questions_and_answers']):
        tool('submit_task_question',{**base,'body':'请调查任务材料中的参数','reason':'需要补查事实','known':'材料在执行成员处'})
    else:
        refs=[];texts=[]
        for f in business['material_sources']:
            texts.append(open(os.path.join(root,f['snapshot_path'])).read());refs.append({'root':0,'path':f['snapshot_path'],'reason':'读取参数'})
        tool('submit_task_result',{**base,'result':json.dumps({'body':'参数核对：'+' | '.join(texts),'files':refs},ensure_ascii=False)})
elif business.get('investigates_question'):
    files=[];texts=[]
    for f in business['material_sources']:
        path=f['snapshot_path'];texts.append(open(os.path.join(root,path)).read());files.append({'root':0,'path':path,'reason':'调查实际读取固定版本'})
    tool('submit_task_result',{**base,'result':json.dumps({'body':'调查结果：'+' | '.join(texts),'files':files},ensure_ascii=False)})
else:
    answers=[q for q in business['questions_and_answers'] if q['revision']==business['revision'] and q.get('answer')]
    if len(answers)<3:
        question=['本地成员是谁？','实验模式标签是什么？','用户要快速还是精确结果？'][len(answers)]
        tool('submit_task_question',{**base,'body':question,'reason':'需要这一事实才能完成选择','known':'已固定两种候选脚本'})
    else:
        files=[];texts=[]
        for f in business['material_sources']:
            path=f['snapshot_path'];texts.append(open(os.path.join(root,path)).read());files.append({'root':0,'path':path,'reason':'实际读取固定版本'})
        emit({'type':'item.completed','item':{'type':'command_execution','command':'read immutable fixture files','exit_code':0,'status':'completed'}})
        body='仅给出选择，还缺参数说明' if len(answers)==3 else '选择 precise.py，参数 iterations=100；fast.py 以速度换精度。依据固定版本：'+' | '.join(texts)
        tool('submit_task_result',{**base,'result':json.dumps({'body':body,'files':files},ensure_ascii=False)})
mcp.stdin.close();assert mcp.wait(timeout=5)==0
emit({'type':'turn.completed'})
