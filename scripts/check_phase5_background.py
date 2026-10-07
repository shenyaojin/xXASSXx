#!/usr/bin/env python3
"""Verify a simulated executor continues after closing a real TUI, with Rust MCP/results."""
import fcntl,json,os,pathlib,pty,re,select,struct,subprocess,sys,termios,time
from phase5_demo import BIN,REPO,run,cli,submit
from phase5_smoke import command,card,task_state

def check(root,evidence):
    run('client','--directory',root/'owner','stop')
    wrapper=root/'slow-codex'
    wrapper.write_text('#!'+sys.executable+'\nimport os,sys,time\nif "exec" in sys.argv: time.sleep(3)\nos.execv(sys.executable,[sys.executable,'+repr(str(REPO/'tests/fixtures/collaborator.py'))+',*sys.argv[1:]])\n');wrapper.chmod(0o700)
    cfg=root/'owner/member.toml';cfg.write_text(re.sub(r'^codex = .*$', 'codex = '+json.dumps(str(wrapper)),cfg.read_text(),flags=re.M));cli(root,'owner','member','configure',cfg)
    run('client','--directory',root/'owner','start')
    i=submit(root,'owner','只读样本，验证关闭界面后仍完成',action='create_task');tid=command(root,'owner',i)['task_id']
    command(root,'owner',submit(root,'owner','授权此目录导入',action='root_add',payload={'path':str(root/'projects/owner')}))
    master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',30,110,0,0));before=termios.tcgetattr(slave)
    child=subprocess.Popen([str(BIN),'open',str(root/'projects/owner'),'--directory',str(root/'owner')],stdin=slave,stdout=slave,stderr=slave,env=dict(os.environ,TERM='xterm-256color'),start_new_session=True)
    data=bytearray()
    def drain(seconds):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if select.select([master],[],[],.03)[0]:
                try:data.extend(os.read(master,65536))
                except OSError:break
    try:
        drain(.8)
        command(root,'owner',submit(root,'owner','仅此 tiny.csv',action='authorize',task=tid,payload={'files':[str(root/'projects/owner/tiny.csv')]}))
        end=time.monotonic()+15
        while time.monotonic()<end:
            drain(.05);c=card(root,'owner',tid)
            if c['state']=='running':break
        assert c['state']=='running',c
        os.write(master,b'\x11');drain(.4);child.wait(timeout=5)
        assert termios.tcgetattr(slave)==before
        final=task_state(root,'owner',tid,'completed')
        assert final['result']['sources'] and 'amount' in final['result']['body']
        evidence.mkdir(parents=True,exist_ok=True)
        result={'kind':'simulated Codex through real MCP','task':tid,'state_at_tui_exit':'running','final_state':final['state'],'sources':final['result']['sources'],'termios_restored':True,'artifact':final['artifact']}
        (evidence/'background.json').write_text(json.dumps(result,ensure_ascii=False,indent=2));print(json.dumps(result,ensure_ascii=False))
    finally:
        if child.poll() is None:child.terminate();child.wait(timeout=5)
        os.close(master);os.close(slave)
if __name__=='__main__':check(pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]))
