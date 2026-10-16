#!/usr/bin/env python3
"""Exercise the foreground launcher and real stdio MCP, with no model calls."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib

args = sys.argv[1:]
assert args[0] == '--cd' and Path(args[1]) == Path.cwd()
assert args[2] == '--config' and len(args) == 4
cfg = tomllib.loads(args[3])['mcp_servers']['xxassxx_butler']
messages = [
    {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': '2025-06-18'}},
    {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
    {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list'},
    {'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call', 'params': {'name': 'butler_context', 'arguments': {}}},
]
out = subprocess.run([cfg['command'], *cfg['args']], cwd=cfg['cwd'],
                     input=''.join(json.dumps(m)+'\n' for m in messages),
                     text=True, capture_output=True, timeout=15, check=True)
responses = [json.loads(line) for line in out.stdout.splitlines()]
assert len(responses) == 3 and 'instructions' in responses[0]['result']
tools = responses[1]['result']['tools']
assert 'collaboration_send_request' in [t['name'] for t in tools]
assert 'submit_task_result' not in [t['name'] for t in tools]
context = responses[2]['result']['structuredContent']
assert not responses[2]['result']['isError']
assert context['identity']['member_id'] == 'owner'
assert context['service']['alive']
assert sys.stdin.isatty() and sys.stdout.isatty()
assert not any(os.environ.get(k) for k in ['OPENAI_API_KEY','CODEX_API_KEY','DEEPSEEK_API_KEY','XXASSXX_MEMBER_TOKEN','XXASSXX_RUN_TOKEN'])
Path('launch-report.json').write_text(json.dumps({'context': context, 'cwd': str(Path.cwd()), 'args': args, 'tty': True}))
sys.exit(17)  # The launcher must preserve Codex's exit status.
