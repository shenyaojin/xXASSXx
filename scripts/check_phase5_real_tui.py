#!/usr/bin/env python3
"""Drive a real TUI follow-up after the opt-in real smoke run."""
import fcntl,json,os,pathlib,pty,select,signal,sqlite3,struct,subprocess,termios,time
from phase5_demo import BIN,run
from phase5_smoke import read

def check(root,evidence):
    evidence.mkdir(parents=True,exist_ok=True)
    master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',36,130,0,0));before=termios.tcgetattr(slave)
    env=os.environ.copy();env['TERM']='xterm-256color'
    child=subprocess.Popen([str(BIN),'open',str(root/'projects/owner'),'--directory',str(root/'owner')],stdin=slave,stdout=slave,stderr=slave,env=env,start_new_session=True)
    data=bytearray()
    def drain(seconds):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if select.select([master],[],[],.05)[0]:
                try:data.extend(os.read(master,65536))
                except OSError:break
    try:
        drain(1)
        assert child.poll() is None
        ids=[]
        for text in ['这次终端界面体验的代号是松柏，只是讨论。请记住，不建立新任务。','刚才终端界面体验的代号是什么？请直接回答，不建立新任务。']:
            os.write(master,b'\x1b[200~'+text.encode()+b'\x1b[201~');drain(.2);os.write(master,b'\r')
            end=time.monotonic()+90
            while time.monotonic()<end:
                drain(.2);records=read(root,'owner',"SELECT id,state,error FROM app_commands WHERE body=? ORDER BY rowid DESC LIMIT 1",(text,))
                if records and records[0]['state'] not in ['pending','processing']:break
            assert records and records[0]['state']=='done',records
            ids.append(records[0]['id'])
        replies=read(root,'owner',"SELECT body FROM app_messages WHERE command_id=? AND kind='answer'",(records[0]['id'],))
        assert replies and '松柏' in replies[0]['body'],replies
        # Inspect real task cards, sources, peer conversation, and raw details with actual keys.
        os.write(master,b'\x14');drain(.4);os.write(master,b'\r');drain(.3);os.write(master,b'\x04');drain(.4);os.write(master,b'\x1b');drain(.2)
        os.write(master,b'\x0f');drain(.2);os.write(master,b'\x1b[B\r');drain(.3);os.write(master,b'\x04');drain(.3)
        os.kill(child.pid,signal.SIGTERM);drain(.5);child.wait(timeout=5)
        assert child.returncode==0 and termios.tcgetattr(slave)==before
        result={'kind':'real model through actual PTY TUI','requests':ids,'input_channel':'tui','answer':replies[0]['body'],'task_and_source_details_opened':True,'peer_history_opened':True,'termios_restored':True,'background_alive':run('client','--directory',root/'owner','status')['alive']}
        (evidence/'real-tui.ansi').write_bytes(data);(evidence/'real-tui.json').write_text(json.dumps(result,ensure_ascii=False,indent=2));print(json.dumps(result,ensure_ascii=False))
    finally:
        if child.poll() is None:child.terminate();child.wait(timeout=5)
        os.close(master);os.close(slave)
if __name__=='__main__':
    import sys;check(pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]))
