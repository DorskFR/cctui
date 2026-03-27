#!/bin/sh
set -e
HOOK_INPUT=$(cat)
echo "$HOOK_INPUT" | python3 -c "
import json, subprocess, sys, os, socket

hook = json.load(sys.stdin)
sid = hook.get('session_id', '')
cwd = hook.get('cwd', os.getcwd())
model = hook.get('model', '')
transcript = hook.get('transcript_path', '')

try:
    branch = subprocess.check_output(
        ['git', '-C', cwd, 'rev-parse', '--abbrev-ref', 'HEAD'],
        stderr=subprocess.DEVNULL
    ).decode().strip()
except Exception:
    branch = 'none'

body = json.dumps(dict(
    claude_session_id=sid,
    machine_id=socket.gethostname(),
    working_dir=cwd,
    metadata=dict(
        git_branch=branch,
        project_name=os.path.basename(cwd),
        model=model,
        transcript_path=transcript,
    ),
))

server = os.environ.get('CCTUI_URL', '__SERVER_URL__')
token = os.environ.get('CCTUI_TOKEN', '__TOKEN__')

subprocess.run([
    'curl', '-sf', '-X', 'POST',
    server + '/api/v1/sessions/register',
    '-H', 'Authorization: Bearer ' + token,
    '-H', 'Content-Type: application/json',
    '-d', body,
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

os.makedirs(os.path.expanduser('~/.cctui'), exist_ok=True)
with open(os.path.expanduser('~/.cctui/session_id'), 'w') as f:
    f.write(sid)

# Start transcript streamer if available
streamer = os.path.expanduser('~/.cctui/bin/streamer.py')
if transcript and os.path.exists(streamer):
    env = os.environ.copy()
    env['CCTUI_URL'] = server
    env['CCTUI_TOKEN'] = token
    subprocess.Popen(
        ['python3', streamer, sid, transcript],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        env=env,
        start_new_session=True,
    )
"
