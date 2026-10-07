#!/usr/bin/env python3
"""Actual PTY check: native Rust TUI, paste, CJK, resize, SIGTERM/SIGINT and restored termios."""
import atexit, re, argparse, fcntl, json, os, pathlib, pty, select, signal, sqlite3, struct, subprocess, termios, time
from phase5_demo import BIN, run

def check(root,evidence):
    evidence.mkdir(parents=True,exist_ok=True)
    db=root/'owner/member.sqlite3'
    def rows():
        with sqlite3.connect(db) as c:return c.execute('SELECT body,recipient FROM app_commands ORDER BY rowid').fetchall()
    initial=rows();captures=[]
    for index,kind in enumerate(['default','-C','open']):
        master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',30,110,0,0));before=termios.tcgetattr(slave)
        env=os.environ.copy();env['TERM']='xterm-256color';env['XDG_DATA_HOME']=str(root/'data')
        # An isolated default-data symlink points only at the test identity, never at a real profile.
        link=root/'data/xxassxx/client';link.parent.mkdir(parents=True,exist_ok=True)
        if not link.exists():link.symlink_to(root/'owner',target_is_directory=True)
        args=[str(BIN)]+([] if kind=='default' else ['-C',str(root/'projects/owner')] if kind=='-C' else ['open',str(root/'projects/owner'),'--directory',str(root/'owner')])
        child=subprocess.Popen(args,stdin=slave,stdout=slave,stderr=slave,cwd=root/'projects/owner',env=env,start_new_session=True)
        atexit.register(lambda c=child: c.poll() is None and c.terminate())
        raw=bytearray()
        def drain(seconds):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.05)[0]:
                    try:raw.extend(os.read(master,65536))
                    except OSError:break
        drain(1.5)
        assert child.poll() is None,raw.decode(errors='replace')
        if index==0:
            assert rows()==initial,'opening must not enqueue inference'
            # First launch now asks about the current directory; this test grants no files.
            if '是否将启动文件夹' in re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',raw.decode(errors='replace')):
                os.write(master,b'n');drain(.3)
            os.write(master,b'\t\x1b[Z');drain(.15)
            os.write(master,b'@');drain(.15);os.write(master,b'\t');drain(.15);os.write(master,b'\x1b');drain(.15)
            text='中文宽字符🙂 引号 "Keith"\n第二行邮箱 a@example.org'
            os.write(master,b'\x1b[200~'+text.encode()+b'\x1b[201~');drain(.3)
            assert rows()==initial,'bracketed paste must not send'
            os.write(master,b'\r');drain(.4)
            assert rows()[-1]==(text,'owner'),'Tab/paste changed recipient or text'
            os.write(master,b'\x1bOP');drain(.2);os.write(master,b'\x1b');drain(.15)
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',12,38,0,0));os.kill(child.pid,signal.SIGWINCH);drain(.2)
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',30,110,0,0));os.kill(child.pid,signal.SIGWINCH);drain(.2)
            os.write(master,b'\x11')
        else:
            assert '中文' in re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',raw.decode(errors='replace')),'reopen must restore persisted history'
            os.kill(child.pid,signal.SIGTERM if index==1 else signal.SIGINT)
        drain(.7);child.wait(timeout=8)
        assert child.returncode==0,(kind,child.returncode,raw.decode(errors='replace')[-1500:])
        assert termios.tcgetattr(slave)==before,'termios not restored'
        assert b'\x1b[?1049l' in raw and b'\x1b[?2004l' in raw
        assert run('client','--directory',root/'owner','status')['alive'] is True
        (evidence/f'{kind.replace("-","")}.ansi').write_bytes(raw)
        captures.append({'entry':kind,'exit':child.returncode,'termios_restored':True,'alternate_screen_restored':True,'daemon_survived':True,'bytes':len(raw)})
        os.close(master);os.close(slave)
    result={'kind':'actual-pty','terminal':'xterm-256color','checks':captures,'paste_unicode_recipient':'passed','resize':'110x30 -> 38x12 -> 110x30','records_persisted':True}
    (evidence/'pty.json').write_text(json.dumps(result,ensure_ascii=False,indent=2));print(json.dumps(result,ensure_ascii=False))
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('root',type=pathlib.Path);p.add_argument('evidence',type=pathlib.Path);a=p.parse_args();check(a.root,a.evidence)
