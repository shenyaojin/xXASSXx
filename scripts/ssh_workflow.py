#!/usr/bin/env python3
"""Explicit REAL Codex + DeepSeek smoke, with bounded budgets and independent host data.

Only deployment/observation: Rust services drive every model turn and peer reply.
--case is an owner-reviewed JSON manifest, not a script; no preset final demo.
Without --case this runs two tiny single-host technical checks only.
"""
import argparse
import json
from pathlib import Path
import secrets
import shlex
import socket
import subprocess
import tempfile
import time
import uuid
import ssh_demo

REPO = Path(__file__).resolve().parents[1]
REMOTE = ssh_demo.REMOTE.replace("timeout=90", "timeout=360")
# Read only explicit artifacts within this fresh test instance. Never read secrets.
REMOTE = REMOTE.replace('elif op in ("cli", "daemon"):', '''elif op == "read":
    path = (root / p["path"]).resolve()
    assert root.resolve() in path.parents and path.stat().st_size <= 32 * 1024 * 1024
    print(json.dumps(path.read_text()))
elif op in ("cli", "daemon"):''')

class Host(ssh_demo.Host):
    def command(self):
        path=shlex.quote(str(Path(self.binary).parent))
        return self.ssh()+['PATH='+path+':"$PATH" python3 -u -c '+shlex.quote(REMOTE)]
    def rpc(self, op, **kw):
        result=subprocess.run(self.command(),input=json.dumps(self.payload(op,**kw))+"\n",
                              text=True,capture_output=True,timeout=390)
        if result.returncode: raise RuntimeError(self.alias+": "+result.stderr)
        return json.loads(result.stdout)

def save(path, value):
    path.write_text(json.dumps(value,ensure_ascii=False,indent=2)+"\n")

def validate_evidence(state, execution):
    assert state["workflow"]["state"]=="completed", state["workflow"]
    assert state["workflow"]["result"]["sources"]
    assert state["file_reads"]
    assert state["model_runs"] and state["model_runs"][0]["state"]=="succeeded"
    names={t["name"] for r in state["model_runs"] for t in r["trace"] if t["result"]["ok"]}
    assert {"inspect_workflow","dispatch_codex"} <= names
    for run in state["runs"]:
        assert run["state"]=="succeeded" and run["turn_completed"] and run["mcp_initialized"]
        assert run["session_id"] and run["reads"]>0 and run["submissions"]>0 and run["exit_code"]==0
    for run in state["runs"][1:]:
        assert run["resumed_session_id"]==state["runs"][0]["session_id"]==run["session_id"]
    # Raw official JSON events, in addition to Rust receipts and process status.
    serialized=json.dumps(execution)
    for token in ["mcp_tool_call","read_task_file","submit_collaboration_turn","turn.completed"]:
        assert token in serialized, token

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host-a",default="trader")
    parser.add_argument("--host-b",default="lakota")
    parser.add_argument("--binary-a",required=True)
    parser.add_argument("--binary-b",required=True)
    parser.add_argument("--codex-a",default="/home/ubuntu/.local/bin/codex")
    parser.add_argument("--codex-b",default="/rcp/rcp42/home/shenyaojin/.nvm/versions/node/v22.18.0/bin/codex")
    parser.add_argument("--model",default="deepseek-flash")
    parser.add_argument("--technical",action="store_true",help="Mark --case as a synthetic protocol check, not the final user demo")
    parser.add_argument("--case",type=Path,help="Reviewed JSON: title, a/b {file,text,goal}")
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--deadline-secs",type=int,default=1200)
    args=parser.parse_args()
    assert 60<=args.deadline_secs<=1800
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    case=json.loads(args.case.read_text()) if args.case else None
    if case:
        for member in ("a","b"):
            assert set(case[member])=={"file","text","goal"}
            assert Path(case[member]["file"]).name==case[member]["file"]
            assert len(case[member]["text"].encode())<=8192
        save(output/"case.json",case)
    name="phase3-real-"+str(uuid.uuid4())
    env=dict(TEAM_A_TOKEN=secrets.token_hex(24),TEAM_B_TOKEN=secrets.token_hex(24),TOKIO_WORKER_THREADS="2")
    report=dict(status="running",run_id=name,mode=("two_host_technical" if args.technical else "reviewed_case") if case else "single_host_technical",
                transport="loopback HTTP through SSH tunnels via Mac/controller; controller must stay connected",
                model=args.model,real_codex=True,real_deepseek=True,checks={},final_demo="not_selected" if not case or args.technical else case["title"])
    masters=[];logs=[];relay=None;services=[];hosts=[];error=None
    controls=tempfile.TemporaryDirectory(prefix="xxworkflow-",dir="/tmp")
    try:
        for label,alias,binary in [("a",args.host_a,args.binary_a),("b",args.host_b,args.binary_b)]:
            host_env={"TOKIO_WORKER_THREADS":"2", "TEAM_"+label.upper()+"_TOKEN":env["TEAM_"+label.upper()+"_TOKEN"]}
            host=Host(alias,binary,name,host_env,Path(controls.name)/label);hosts.append(host)
            log=open(output/(label+"-ssh.log"),"w");logs.append(log)
            master=subprocess.Popen(host.ssh("-M","-N","-o","ControlPersist=no"),stdout=log,stderr=log);masters.append(master)
            deadline=time.monotonic()+25
            while not Path(host.control).exists():
                if master.poll() is not None or time.monotonic()>deadline: raise RuntimeError("SSH failed: "+alias)
                time.sleep(0.1)
            host.info=host.rpc("setup")
        a,b=hosts
        report["hosts"]={label:dict(alias=h.alias,**h.info) for label,h in zip(("a","b"),hosts)}
        assert a.info["binary_sha256"]==b.info["binary_sha256"]
        a.write("mailbox.toml",(REPO/"examples/mailbox.toml").read_text())
        port=a.rpc("port")
        relay_host=Host(a.alias,a.binary,a.name,env,a.control)
        relay=ssh_demo.Service(relay_host,output,"mailbox",["mailbox","serve","--listen",f"127.0.0.1:{port}","--config",a.info["root"]+"/mailbox.toml"],db="relay.sqlite3")
        relay.wait_for("started_pid");relay.wait_for("listening")
        with socket.socket() as sock:
            sock.bind(("127.0.0.1",0));local=sock.getsockname()[1]
        a.forward("-L",f"127.0.0.1:{local}:127.0.0.1:{port}")
        port_b=b.rpc("port");b.forward("-R",f"127.0.0.1:{port_b}:127.0.0.1:{local}")
        materials={};workflows={}
        for member,h,peer,p,codex in [("a",a,"b",port,args.codex_a),("b",b,"a",port_b,args.codex_b)]:
            h.call("member","init","--id",member,"--name",h.alias,"--team","lab")
            home=h.info["root"].split("/xxassxx-tests/")[0]
            config=f'''mailbox_url="http://127.0.0.1:{p}"
credential_env="TEAM_{member.upper()}_TOKEN"
[[contacts]]
member_id="{peer}"
display_name="{peer}"
[executor]
mode="auto"
codex={json.dumps(codex)}
workdir="work"
timeout_secs=240
[model]
provider="deepseek"
model={json.dumps(args.model)}
secrets_file={json.dumps(home+"/.config/xxassxx/secrets.env")}
api_key_env="DEEPSEEK_API_KEY"
timeout_secs=60
max_model_calls=4
max_tool_rounds=3
max_tokens=1024
'''
            h.write("member.toml",config);h.call("member","configure",h.info["root"]+"/member.toml")
            nonce=secrets.token_hex(6)
            material=case[member] if case else dict(file="probe.json",text=json.dumps({"probe":nonce,"values":[7,11,13]}),goal="Read the authorized JSON through task MCP. Submit a completed JSON body with probe copied exactly and total equal to the sum of values. Cite the read source.")
            materials[member]=material
            # A single text document retains immutable record.txt; manifest file label is descriptive.
            obj=h.call("object","create","--title",material["file"],"--kind","dataset")
            ver=h.call("object","publish",obj["id"],"--expected","none","--request-id","initial","--text",material["text"])
            materials[member]=dict(material,object_id=obj["id"],version_id=ver["id"],grant=ver["id"]+":record.txt")
        if case:
            workflows["b"]=b.call("collaboration","prepare","--peer","a","--goal",materials["b"]["goal"],"--grant",materials["b"]["grant"])
            workflows["a"]=a.call("collaboration","start","--peer","b","--remote-workflow",workflows["b"]["id"],"--goal",materials["a"]["goal"],"--grant",materials["a"]["grant"])
        else:
            for label,h in zip(("a","b"),hosts):
                workflows[label]=h.call("collaboration","local","--goal",materials[label]["goal"],"--grant",materials[label]["grant"])
        report["workflows"]={k:w["id"] for k,w in workflows.items()}
        report["materials"]={k:{f:v for f,v in m.items() if f!="text"} for k,m in materials.items()}
        save(output/"report.json",report)
        for h in hosts:
            h.call("service","start","--poll-secs","2");services.append(h)
        deadline=time.monotonic()+args.deadline_secs;last=None
        while time.monotonic()<deadline:
            states={label:h.call("collaboration","show",workflows[label]["id"]) for label,h in zip(("a","b"),hosts)}
            current={label:s["workflow"]["state"] for label,s in states.items()}
            for label,s in states.items(): save(output/(label+"-workflow.json"),s)
            if current!=last:print(json.dumps(current),flush=True);last=current
            if any(s in ("failed","needs_attention","limit_reached","timed_out","stopped") for s in current.values()):raise RuntimeError("Workflow stopped: "+json.dumps(current))
            if all(s=="completed" for s in current.values()):break
            time.sleep(3)
        else: raise TimeoutError("Real workflow observation deadline")
        for label,h in zip(("a","b"),hosts):
            s=states[label];execution=h.call("show",s["workflow"]["task_id"],"--events")
            save(output/(label+"-execution.json"),execution);validate_evidence(s,execution)
            artifact=h.rpc("read",path="member.sqlite3.results/"+workflows[label]["id"]+".json")
            (output/(label+"-result.json")).write_text(artifact)
            if not case:
                body=s["workflow"]["result"]["body"]
                if body.startswith("```"):body="\n".join(body.splitlines()[1:-1])
                result=json.loads(body);assert result["probe"]==json.loads(materials[label]["text"])["probe"] and result["total"]==31
            report["checks"][label+"_real_tools_and_persisted_result"]=True
        if case:
            events=[m["workflow"]["event"] for m in states["a"]["messages"]]
            assert events[0]=="request" and events[-1]=="result",events
            cursor=iter(events)
            assert all(any(x==required for x in cursor) for required in ["request","clarification","answer","result"]),events
            assert len(states["a"]["runs"])>=3 and len(states["b"]["runs"])>=2
            report["checks"]["automatic_clarification_and_explicit_session_resume"]=True
        # Read-only queries and idle ticks must leave model/execution counts stable.
        time.sleep(3)
        for label,h in zip(("a","b"),hosts):
            after=h.call("collaboration","show",workflows[label]["id"])
            assert after["runs"]==states[label]["runs"] and after["model_runs"]==states[label]["model_runs"]
        report["checks"]["idle_no_model_calls"]=True;report["status"]="passed"
    except Exception as e:
        error=e;report["status"]="failed";report["error"]=str(e)
    finally:
        report["cleanup"]={}
        for h in services:
            try:
                # Revoke pending/in-flight capability on failed validation before stopping service.
                if error:
                    for w in h.call("collaboration","list"):
                        if w["state"]!="completed":h.call("collaboration","stop",w["id"])
                stopped=h.call("service","stop","--wait-secs","300")
                report["cleanup"][h.alias]=stopped
                save(output/(h.alias+"-service-log.json"),h.call("service","logs","--lines","200"))
                if stopped["alive"]:raise RuntimeError("Service still alive")
            except Exception as e:report["cleanup"][h.alias]={"error":str(e)};report["status"]="failed";error=error or e
        if relay:
            try:relay.stop();report["cleanup"]["mailbox"]="stopped"
            except Exception as e:report["status"]="failed";error=error or e
        for master in masters:
            master.terminate()
            try:master.wait(timeout=10)
            except subprocess.TimeoutExpired:master.kill();master.wait()
        for log in logs:log.close()
        controls.cleanup();save(output/"report.json",report)
    print(str(output/"report.json"),flush=True)
    if error:raise SystemExit(str(error))
if __name__=="__main__":main()
