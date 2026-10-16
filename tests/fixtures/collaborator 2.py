#!/usr/bin/env python3
"""Simulation only. Reads actual authorized bytes through the Rust MCP."""
import csv
import io
import json
import os
import re
import subprocess
import sys
import time
import tomllib
import uuid

args=sys.argv[1:]
if args==['--version']:
    print('codex-cli simulated-collaboration');sys.exit(0)
if args==['login','status']:
    print('Logged in using ChatGPT');sys.exit(0)
mode=os.environ.get('XXASSXX_WORKFLOW_MODE','success')
config={}
for i,arg in enumerate(args):
    if arg=='-c':
        key,value=args[i+1].split('=',1);config[key]=tomllib.loads('value='+value)['value']
assert config['sandbox_mode']=='read-only' and not config['features.shell_tool']
assert not any(k.startswith('mcp_servers.xxassxx_butler.') for k in config)
assert not os.environ.get('DEEPSEEK_API_KEY')
assert not os.environ.get('DEEPSEEK_API')
prompt=sys.stdin.read()
task=re.search(r'Assigned task_id: ([a-f0-9-]+)',prompt)[1]
run=re.search(r'Current run_id: ([a-f0-9-]+)',prompt)[1]
session=str(uuid.uuid5(uuid.NAMESPACE_DNS,task))
if 'resume' in args:
    assert args[args.index('resume')+1]==session
    if mode=='wrong_session':session=str(uuid.uuid4())
def emit(data):print(json.dumps(data),flush=True)
emit({'type':'thread.started','thread_id':session});emit({'type':'turn.started'})
mcp=subprocess.Popen([config['mcp_servers.xxassxx.command'],*config['mcp_servers.xxassxx.args']],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,env=os.environ.copy())
sequence=0
def rpc(method,params=None,notify=False):
    global sequence
    sequence+=1;req={'jsonrpc':'2.0','method':method,'params':params or {}}
    if not notify:req['id']=sequence
    mcp.stdin.write(json.dumps(req)+'\n');mcp.stdin.flush()
    if notify:return
    return json.loads(mcp.stdout.readline())['result']
def tool(name,params):
    r=rpc('tools/call',{'name':name,'arguments':params})
    assert not r.get('isError'),r
    emit({'type':'item.completed','item':{'type':'mcp_tool_call','server':'xxassxx','tool':name,'status':'completed','result':r}})
    return r['structuredContent']
rpc('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'simulated-collaborator','version':'1'}})
rpc('notifications/initialized',notify=True)
ctx=json.loads(tool('get_task',{'task_id':task})['input'])
files=tool('list_task_files',{})['files']
texts=[];sources=[]
for f in files:
    offset=0;text=''
    while True:
        chunk=tool('read_task_file',{'version_id':f['version_id'],'path':f['path'],'offset':offset,'max_bytes':64})
        text+=chunk['text'];offset=chunk['next_offset']
        if chunk['eof']:break
    texts.append(text);sources.append(chunk['source'])
if mode=='hold':
    with open(os.environ['XXASSXX_WORKFLOW_PID'],'w') as handle:json.dump({'codex':os.getpid(),'mcp':mcp.pid},handle)
    time.sleep(30)
current=next((m for m in ctx['conversation'] if m['message_id']==ctx['current_message_id']),None)
reply_to=None
if ctx['role']=='local':
    action='complete';body=json.dumps({'observed_text':texts[0]})
elif ctx['role']=='owner':
    local=json.loads(texts[0])
    if current is None:
        action='ask';body='Please compute the adjusted total from your authorized CSV; ask me if the adjustment is unclear.'
    elif current['workflow']['event']=='clarification':
        action='reply';reply_to=current['message_id'];body=json.dumps({'factor':local['factor']})
    else:
        remote=json.loads(current['body']);sources+=current['workflow']['sources'];action='complete';body=json.dumps({'final_total':remote['adjusted']+local['fee']})
else:
    if current['workflow']['event']=='request':
        action='ask';body='Which factor applies to this dataset? Please use your local rules.'
    else:
        factor=json.loads(current['body'])['factor'];total=sum(int(r['amount']) for r in csv.DictReader(io.StringIO(texts[0])))
        action='complete';body=json.dumps({'adjusted':factor*total});sources+=current['workflow']['sources']
# Source refs can repeat through a conversation; normalize only test output.
sources=list({json.dumps(s,sort_keys=True):s for s in sources}.values())
outcome={'task_id':task,'run_id':run,'idempotency_key':run,'action':action,'body':body,'sources':sources}
if reply_to:outcome['reply_to']=reply_to
tool('submit_collaboration_turn',outcome)
if mode=='duplicate':tool('submit_collaboration_turn',outcome)
mcp.stdin.close();assert mcp.wait(timeout=5)==0
if mode=='fail_after_result':
    emit({'type':'turn.failed','error':{'message':'simulated failure after staged collaboration turn'}});sys.exit(7)
emit({'type':'turn.completed'})
