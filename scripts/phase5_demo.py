#!/usr/bin/env python3
"""Isolated local Phase 5 experience. Python only sets up fixtures and drives Rust APIs."""
import argparse, json, os, pathlib, shlex, socket, subprocess, tempfile, tomllib, uuid
REPO=pathlib.Path(__file__).resolve().parents[1]
BIN=REPO/'target/debug/xxassxx'
def run(*args, data=None):
    p=subprocess.run([str(BIN),*map(str,args)],input=None if data is None else json.dumps(data),text=True,capture_output=True)
    if p.returncode: raise RuntimeError(p.stderr.strip())
    return json.loads(p.stdout)
def literal(v):
    if isinstance(v,bool): return str(v).lower()
    if isinstance(v,(str,int,float)):return json.dumps(v,ensure_ascii=False)
    if isinstance(v,list):return '['+','.join(map(literal,v))+']'
    raise ValueError('unsupported config value')
def table(name,fields): return '\n['+name+']\n'+''.join(k+' = '+literal(v)+'\n' for k,v in fields.items() if v is not None)
def cli(root,member,*args,data=None):return run('--db',root/member/'member.sqlite3',*args,data=data)
def session(root,member,peer=None):return cli(root,member,'app','session','--project',root/'projects'/member,'--recipient',peer or member)['session_id']
def submit(root,member,body,action='chat',task=None,payload=None,peer=None,channel='test-adapter',session_id=None):
    i=dict(request_id=str(uuid.uuid4()),channel=channel,session_id=session_id or session(root,member,peer),task_id=task,recipient=peer or member,body=body,action=action,payload=payload or {})
    cli(root,member,'app','submit',data=i);return i

def prepare(root,real):
    root.mkdir(mode=0o700,parents=True,exist_ok=False)
    with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
    run('server','--directory',root/'server','init','--team','phase5-isolated','--public-url',f'http://127.0.0.1:{port}','--member','owner=沈尧 Shenyao "Keith" Jin','--member','test=测试管家')
    cfg=None
    if real:
        source=pathlib.Path(os.environ.get('XXASSXX_DEMO_MODEL_CONFIG',str(pathlib.Path.home()/'.local/share/xxassxx/client/member.toml')))
        cfg=tomllib.loads(source.read_text())
        if not cfg['model'].get('secrets_file'):raise RuntimeError('真实体验要求私有配置中的 model.secrets_file；不会读取/复制凭据或从正文取得密钥。')
    for m in ['owner','test']:
        p=root/'projects'/m;p.mkdir(parents=True)
        (p/'tiny.csv').write_text('item,amount\nx,4\ny,5\n')
        (p/'read_me.py').write_text('# Synthetic Phase 5 fixture; never run this script.\nFACTOR = 3\n')
        run('client','--directory',root/m,'init','--invite',root/'server/invitations'/f'{m}.json','--provider','off','--codex',cfg['executor'].get('codex','codex') if cfg else '/not-installed-phase5-no-automatic-execution')
        if cfg:
            dest=root/m/'member.toml';local=tomllib.loads(dest.read_text());model=cfg['model'].copy();model.update(timeout_secs=45,max_model_calls=4,max_tool_rounds=3,max_tool_calls=10,max_tokens=1536)
            text=dest.read_text().split('[model]')[0]+table('model',model)
            dest.write_text(text);dest.chmod(0o600);cli(root,m,'member','configure',dest)
        session(root,m)
    obj=cli(root,'test','object','create','--title','Synthetic tiny CSV','--kind','dataset','--shared')
    cli(root,'test','object','publish',obj['id'],'--expected','none','--request-id',str(uuid.uuid4()),'--text','item,amount\nx,4\ny,5\n','--note','Only synthetic data; metadata is explicitly shared.')
    with open(root/'mailbox.log','ab') as log:
        mailbox=subprocess.Popen([str(BIN),'server','--directory',str(root/'server'),'serve','--listen',f'127.0.0.1:{port}'],stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
    (root/'mailbox.pid').write_text(str(mailbox.pid))
    for m in ['owner','test']:run('client','--directory',root/m,'start')
    info={'root':str(root),'mode':'real' if real else 'model-off','binary':str(BIN),'members':['owner','test'],'production_services_changed':False}
    (root/'environment.json').write_text(json.dumps(info,ensure_ascii=False,indent=2))
    for m in ['owner','test']:
        command=shlex.join([str(BIN),'open',str(root/'projects'/m),'--directory',str(root/m)])
        (root/f'open-{m}.sh').write_text('#!/bin/sh\nexec '+command+'\n');(root/f'open-{m}.sh').chmod(0o700)
    (root/'stop.sh').write_text('#!/bin/sh\nexec '+shlex.join([os.sys.executable,str(pathlib.Path(__file__).resolve()),'--stop',str(root)])+'\n')
    (root/'stop.sh').chmod(0o700)
    return info

def stop(root):
    info=json.loads((root/'environment.json').read_text())
    if pathlib.Path(info['root'])!=root or info.get('production_services_changed') is not False:raise RuntimeError('Not a Phase 5 isolated environment')
    for member in ['owner','test']:run('client','--directory',root/member,'stop')
    pid=int((root/'mailbox.pid').read_text())
    status=subprocess.run(['ps','-p',str(pid),'-o','command='],capture_output=True,text=True)
    expected=' '.join([str(BIN),'server','--directory',str(root/'server'),'serve','--listen'])
    if status.returncode==0 and status.stdout.strip().startswith(expected+' '):
        import signal
        os.kill(pid,signal.SIGTERM)
    print('隔离环境已停止；数据与结果保留在 '+str(root))

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--stop',type=pathlib.Path);p.add_argument('--real',action='store_true',help='Reuse saved model config and official Codex login; calls occur only after a user command.');p.add_argument('--prepare-only',action='store_true');p.add_argument('--root',type=pathlib.Path);p.add_argument('--smoke',action='store_true',help='Explicitly run bounded real model/Codex acceptance tests.');args=p.parse_args()
    if args.stop:
        stop(args.stop.resolve());return
    if not BIN.is_file():raise SystemExit('Build first: cargo build --locked')
    root=args.root or pathlib.Path(tempfile.gettempdir())/('xxassxx-phase5-'+uuid.uuid4().hex[:10]);root=root.resolve()
    print(json.dumps(prepare(root,args.real),ensure_ascii=False,indent=2),flush=True)
    print('另一个终端打开 test：'+str(root/'open-test.sh')+'\n停止此隔离环境：'+str(root/'stop.sh'),flush=True)
    if args.smoke:
        if not args.real:raise SystemExit('--smoke requires --real')
        from phase5_smoke import smoke
        smoke(root)
    elif not args.prepare_only:
        if not args.real:print('当前模型关闭：可以查看界面、身份、联系人；自然聊天请重新创建 --real 环境。',flush=True)
        subprocess.run([str(root/'open-owner.sh')],check=True)
if __name__=='__main__':main()
