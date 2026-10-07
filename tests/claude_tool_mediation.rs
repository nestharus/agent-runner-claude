//! `oulipoly.tool_mediation/v1` through the real provider binary: a fake
//! Claude Code CLI that starts the MCP servers its `--mcp-config` names and
//! calls their `bash` tool, a requester stand-in speaking the root Bash
//! ingress wire, and a stand-in root ingress socket that records each request
//! and its peer. Fakes establish plumbing only: whether a real Claude Code
//! honours the constructed options is not shown here.
#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);
const INGRESS_ENV: &str = "OULIPOLY_ROOT_BASH_V1";

/// Fake `claude -p` in stream-JSON mode. A prompt `bash <command>` makes it
/// start every server of its `--mcp-config` (environment: only PATH and HOME
/// plus the server's own `env`, so nothing reaches the server implicitly),
/// list tools and call `bash` with that command, then say the tool's answer.
const FAKE_CLAUDE: &str = r#"#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
line = sys.stdin.readline()
message = json.loads(line)
prompt = message['message']['content'][0]['text']
opts = {}
i = 0
while i < len(args):
    if args[i] in ['--mcp-config', '--tools', '--allowedTools', '--disallowedTools', '--permission-mode', '--setting-sources', '--resume', '--session-id', '--model', '--input-format', '--output-format']:
        opts[args[i]] = args[i + 1]; i += 2
    else:
        i += 1
record = {'argv': args, 'opts': opts, 'tool_search': os.environ.get('ENABLE_TOOL_SEARCH'), 'pid': os.getpid(), 'offer_in_native_env': 'OULIPOLY_EXPLORATION_V1' in os.environ}
session = opts.get('--resume', opts.get('--session-id'))
def emit(event):
    print(json.dumps(event), flush=True)
init = {'type': 'system', 'subtype': 'init', 'session_id': session}
if prompt.startswith('inventory '):
    init.update(tools=opts['--allowedTools'].split(','), mcp_servers=[{'name':'oulipoly','status':'connected'}])
    case = prompt.split(' ', 1)[1]
    if case == 'extra-tool': init['tools'].append('Bash')
    elif case == 'missing-tool': init['tools'] = []
    elif case == 'extra-server': init['mcp_servers'].append({'name':'foreign','status':'connected'})
    elif case == 'disconnected': init['mcp_servers'][0]['status'] = 'failed'
    elif case == 'invalid': init['tools'] = None
    elif case == 'no-explore': init['tools'] = [t for t in init['tools'] if t != 'mcp__oulipoly__explore']
emit(init)
emit({'type': 'user', 'uuid': message['uuid'], 'session_id': session, 'parent_tool_use_id': None, 'message': message['message']})
said = 'no tool'
words = prompt.split(' ', 1)
if words[0] == 'explore' and 'mcp__oulipoly__explore' not in opts.get('--allowedTools', '').split(','):
    said = 'explore is not an allowed tool'
    servers = json.loads(opts.get('--mcp-config', '{"mcpServers":{}}'))['mcpServers']
    record['servers'] = list(servers)
    record['offer_in_mcp_config'] = any('OULIPOLY_EXPLORATION_V1' in s.get('env', {}) for s in servers.values())
    with open(os.environ['CALLS'], 'a') as f:
        f.write(json.dumps(record) + '\n')
elif words[0] in ('bash', 'explore'):
    servers = json.loads(opts['--mcp-config'])['mcpServers']
    record['servers'] = list(servers)
    server = servers['oulipoly']
    env = {'PATH': os.environ.get('PATH', ''), 'HOME': os.environ.get('HOME', '')}
    env.update(server.get('env', {}))
    record['offer_in_mcp_config'] = 'OULIPOLY_EXPLORATION_V1' in server.get('env', {})
    mcp = subprocess.Popen([server['command']] + server['args'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True)
    def rpc(id, method, params=None):
        mcp.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': id, 'method': method, 'params': params or {}}) + '\n'); mcp.stdin.flush()
        return json.loads(mcp.stdout.readline())
    rpc(1, 'initialize', {'protocolVersion': '2025-06-18'})
    record['tools'] = [t['name'] for t in rpc(2, 'tools/list')['result']['tools']]
    record['mcp_pid'] = mcp.pid
    with open(os.environ['CALLS'], 'a') as f:
        f.write(json.dumps(record) + '\n')
    if words[0] == 'bash':
        result = rpc(3, 'tools/call', {'name': 'bash', 'arguments': {'command': words[1]}})['result']
    else:
        route, question = words[1].split(' ', 1)
        arguments = {'question': question}
        if route != '-':
            arguments['route'] = route
        result = rpc(3, 'tools/call', {'name': 'explore', 'arguments': arguments})['result']
    record['is_error'] = result['isError']
    said = result['content'][0]['text']
    mcp.stdin.close(); mcp.wait()
else:
    with open(os.environ['CALLS'], 'a') as f:
        f.write(json.dumps(record) + '\n')
emit({'type': 'assistant', 'parent_tool_use_id': None, 'message': {'content': [{'type': 'text', 'text': said}]}})
emit({'type': 'result', 'subtype': 'success', 'is_error': False, 'session_id': session})
"#;

/// Requester stand-in: the root ingress wire of `agent-bash run`, reduced to
/// what the bridge reads (one `agent-bash-root-v1` result object).
const REQUESTER: &str = r#"#!/usr/bin/env python3
import base64, json, os, socket, sys
args = sys.argv[1:]
assert args[:2] == ['run', '--delivery'] and args[3] == '--', args
argv = args[4:]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(os.environ['OULIPOLY_ROOT_BASH_V1'])
s.sendall((json.dumps({'v': 1, 'op': 'run', 'argv': argv, 'cwd': os.getcwd()}) + '\n').encode())
stages, out, end = [], b'', None
for line in s.makefile('rb'):
    event = json.loads(line)
    stages.append({'event': event['event']})
    if event['event'] == 'output':
        out += base64.b64decode(event['b64'])
    if event['event'] in ('end', 'refused'):
        end = event; break
if end is None:
    print(json.dumps({'result_surface': 'agent-bash-root-v1', 'version': 1, 'delivery_mode': 'sync', 'outcome': 'unknown', 'meaning': 'accepted-end-unknown', 'effects_possible': True, 'stages': stages, 'faults': [], 'wait': None, 'output': {'base64': '', 'bytes': 0, 'delivery': 'unproven'}}))
elif end['event'] == 'refused':
    print(json.dumps({'result_surface': 'agent-bash-root-v1', 'version': 1, 'delivery_mode': 'sync', 'outcome': 'refused', 'effects_possible': False, 'refusal': {'by': 'owner', 'reason': end['reason']}, 'stages': stages, 'faults': [], 'wait': None, 'output': {'base64': '', 'bytes': 0, 'delivery': 'none'}}))
else:
    print(json.dumps({'result_surface': 'agent-bash-root-v1', 'version': 1, 'delivery_mode': 'sync', 'outcome': 'ended', 'effects_possible': True, 'stages': stages, 'faults': [], 'wait': {'status': end['status'], 'observer': 'work-pid1-wait', 'exit': {'code': int(end['status'].split(':')[1])}}, 'output': {'base64': base64.b64encode(out).decode(), 'bytes': len(out), 'delivery': 'complete'}}))
"#;

/// Child requester stand-in (root-child v1 requester surface, reduced):
/// `REQUESTER ROUTE QUESTION` asks the owner named by its ingress for one
/// child, relays the owner's stages on stderr and prints its final object.
const CHILD_REQUESTER: &str = r#"#!/usr/bin/env python3
import json, os, socket, sys
route, question = sys.argv[1:]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(os.environ['OULIPOLY_ROOT_BASH_V1'])
s.sendall((json.dumps({'v': 1, 'op': 'child', 'route': route, 'prompt': question, 'cwd': os.getcwd()}) + '\n').encode())
for line in s.makefile('rb'):
    event = json.loads(line)
    if event['event'] == 'result':
        print(json.dumps(event)); sys.exit(0)
    print('%s: %s' % (event['event'], json.dumps(event)), file=sys.stderr, flush=True)
print(json.dumps({'event': 'lost'})); sys.exit(75)
"#;

/// One request the stand-in ingress received.
#[derive(Debug, Clone)]
struct Seen {
    request: Value,
    peer: i32,
    /// The peer's ancestors when it connected, nearest first.
    ancestry: Vec<i32>,
    eof_while_running: bool,
}

/// Stand-in root Bash ingress: answers `hang` with accepted/started and then
/// waits for its requester to go away; anything else ends with code 0 and
/// output naming the command and directory.
struct Ingress {
    path: PathBuf,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn peer_pid(stream: &UnixStream) -> i32 {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid socket descriptor and an out-buffer of the stated size.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    assert_eq!(rc, 0);
    cred.pid
}

impl Ingress {
    fn start(dir: &Path) -> Self {
        let path = dir.join("bash.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let record = Arc::clone(&record);
                std::thread::spawn(move || {
                    let peer = peer_pid(&stream);
                    let ancestry = ancestors(peer);
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let index = {
                        let mut seen = record.lock().unwrap();
                        seen.push(Seen {
                            request: request.clone(),
                            peer,
                            ancestry,
                            eof_while_running: false,
                        });
                        seen.len() - 1
                    };
                    let command = request["argv"][2].as_str().unwrap_or_default().to_owned();
                    let mut say = |event: Value| {
                        let _ = writeln!(stream, "{event}");
                    };
                    if request["op"] == json!("child") {
                        let child = format!("child-{}", index + 1);
                        say(json!({"event":"accepted","child":child,"durable":true}));
                        if request["prompt"] == json!("hang") {
                            let mut rest = String::new();
                            let _ = reader.read_line(&mut rest);
                            record.lock().unwrap()[index].eof_while_running = true;
                            return;
                        }
                        say(
                            json!({"event":"result","child":child,"route":request["route"],
                            "outcome":"answered","answer":format!("answer to {}", request["prompt"].as_str().unwrap()),
                            "turn_end":{"stop_reason":"end_turn"},"stopped":null,"launch":null,
                            "end":{"event":"end","status":"code:0","observer":"work-pid1-wait","namespace":{"drained":true}},
                            "lifecycle":{"end":"observed","bash_runs_open":0,"bash_run_end_unknown":false,"budget":"released"}}),
                        );
                        return;
                    }
                    say(
                        json!({"event":"accepted","root_id":"stand-in","work":index + 1,"durable":true}),
                    );
                    say(json!({"event":"started","work":index + 1}));
                    if command == "hang" {
                        let mut rest = String::new();
                        let _ = reader.read_line(&mut rest);
                        record.lock().unwrap()[index].eof_while_running = true;
                        return;
                    }
                    let output = format!("ran {command} in {}\n", request["cwd"].as_str().unwrap());
                    say(
                        json!({"event":"output","b64":agent_provider_execution::encoding::encode_base64(output.as_bytes())}),
                    );
                    say(json!({"event":"output-closed","bytes":output.len()}));
                    say(
                        json!({"event":"end","status":"code:0","observer":"work-pid1-wait","output":{"state":"closed"}}),
                    );
                });
            }
        });
        Self { path, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// The parent chain of `pid`, nearest first.
fn ancestors(pid: i32) -> Vec<i32> {
    let mut chain = Vec::new();
    let mut pid = pid;
    while pid > 1 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let Some((_, after)) = stat.rsplit_once(')') else {
            break;
        };
        pid = after
            .split_whitespace()
            .nth(1)
            .and_then(|ppid| ppid.parse().ok())
            .unwrap_or(0);
        chain.push(pid);
    }
    chain
}

fn alive(pid: i64) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| !stat.contains(") Z "))
}

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("u94-correction-claude-mediation-")
            .tempdir_in("/tmp")
            .unwrap();
        for (name, text) in [
            ("claude", FAKE_CLAUDE),
            ("requester", REQUESTER),
            ("child-requester", CHILD_REQUESTER),
        ] {
            let path = root.path().join(name);
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::create_dir(root.path().join("work")).unwrap();
        Self { root }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn policy(&self, bash: Value) -> String {
        json!({"protocol":"oulipoly.tool_mediation/v1","bash":bash,
            "requester":self.path().join("requester"),"ingress_env":INGRESS_ENV})
        .to_string()
    }

    fn offer(&self, routes: &[&str]) -> String {
        json!({"protocol":"oulipoly.exploration/v1","routes":routes,
            "requester":self.path().join("child-requester"),"ingress_env":INGRESS_ENV,
            "limits":{"max_starts":4}})
        .to_string()
    }

    /// A host that also selects exploration/v1.
    fn exploring_host(&self) -> Value {
        let mut host = self.host();
        host["env"]["OULIPOLY_HOST_EXPLORATION_V1"] = json!("1");
        host
    }

    /// A mediated template, with `offer` beside the policy when given.
    fn offered(&self, offer: Option<String>) -> Value {
        let mut template = self.template(&[], Some(self.policy(json!({"allow":["true"]}))));
        if let Some(offer) = offer {
            template["env"]["OULIPOLY_EXPLORATION_V1"] = json!(offer);
        }
        template
    }

    fn evaluate(&self, host: Value, env: Value) -> Value {
        self.invoke(
            "policy.evaluate",
            host,
            json!({"settings_id":"claude-primary","mode":"headless",
                "model":{"name":"opus","provider_args":[],"inputs":{"prompt":"hi","named":{}}},
                "launch":{"command":"claude","env":env}}),
        )["result"]
            .clone()
    }

    fn host(&self) -> Value {
        json!({"app":"oulipoly-agent-runner","working_directory":self.path(),
            "data_root":self.path().join("data"),
            "env":{"OULIPOLY_HOST_RESIDENT_SESSION_V1":"1","OULIPOLY_HOST_TOOL_MEDIATION_V1":"1"}})
    }

    fn template(&self, argv_extra: &[&str], policy: Option<String>) -> Value {
        let mut argv = vec![
            self.path().join("claude").display().to_string(),
            "--model".into(),
            "opus".into(),
        ];
        argv.extend(argv_extra.iter().map(|arg| (*arg).to_owned()));
        let mut env = json!({"CALLS": self.path().join("calls.jsonl")});
        if let Some(policy) = policy {
            env["OULIPOLY_TOOL_MEDIATION_V1"] = json!(policy);
        }
        json!({"settings_id":"claude-primary","mode":"headless",
            "model":{"name":"claude-opus","provider_args":[],"inputs":{"prompt":null,"named":{}}},
            "argv":argv,"env":env})
    }

    fn invoke(&self, operation: &str, host: Value, params: Value) -> Value {
        let request = json!({"contract":"oulipoly.provider/v1","request_id":format!("req-{operation}"),
            "provider_instance_id":"claude-primary","host":host,"params":params});
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"))
            .arg(operation)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        serde_json::from_str(text.lines().last().unwrap_or("null")).unwrap_or(json!(text))
    }

    fn prepare(&self, template: Value) -> Value {
        self.prepare_for(self.host(), template)
    }

    fn prepare_for(&self, host: Value, template: Value) -> Value {
        self.invoke(
            "resident.prepare",
            host,
            json!({"protocol":"oulipoly.resident_session/v1","launch":template}),
        )
    }

    fn calls(&self) -> Vec<Value> {
        std::fs::read_to_string(self.path().join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<Value>,
    seen: Vec<Value>,
    next_id: u64,
}

impl Client {
    fn serve(prepared: &Value, ingress: Option<&Path>) -> Self {
        Self::serve_with(prepared, ingress, &[])
    }

    /// Serves with `inherited` in the endpoint's own process environment.
    fn serve_with(prepared: &Value, ingress: Option<&Path>, inherited: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"));
        for arg in prepared["invocation"]["args"].as_array().unwrap() {
            command.arg(arg.as_str().unwrap());
        }
        command.env_remove(INGRESS_ENV);
        command.env_remove("OULIPOLY_EXPLORATION_V1");
        command.envs(inherited.iter().copied());
        if let Some(ingress) = ingress {
            command.env(INGRESS_ENV, ingress);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, messages) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if send
                    .send(serde_json::from_str::<Value>(&line).unwrap())
                    .is_err()
                {
                    return;
                }
            }
        });
        let stdin = child.stdin.take();
        Self {
            child,
            stdin,
            messages,
            seen: Vec::new(),
            next_id: 0,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        stdin.flush().unwrap();
        id
    }

    fn wait(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.seen.iter().position(&pred) {
            return self.seen.remove(index);
        }
        let start = Instant::now();
        loop {
            let left = TIMEOUT
                .checked_sub(start.elapsed())
                .unwrap_or_else(|| panic!("timed out waiting for {what}; seen {:?}", self.seen));
            let message = self
                .messages
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no {what}; seen {:?}", self.seen));
            if pred(&message) {
                return message;
            }
            self.seen.push(message);
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        self.response(id)
    }

    fn notify(&mut self, method: &str, params: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","method":method,"params":params})
        )
        .unwrap();
        stdin.flush().unwrap();
    }

    fn response(&mut self, id: u64) -> Value {
        self.wait(&format!("response {id}"), |m| {
            m.get("method").is_none() && m["id"] == json!(id)
        })
    }

    fn update(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        self.wait(what, |m| {
            m["method"] == json!("session/update") && pred(&m["params"]["update"])
        })["params"]["update"]
            .clone()
    }

    fn open(&mut self, cwd: &Path) -> String {
        self.call(
            "initialize",
            json!({"protocolVersion":2,"info":{"name":"t","version":"0"}}),
        );
        self.call("session/new", json!({"cwd":cwd}))["result"]["sessionId"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn prompt(&mut self, session: &str, text: &str) -> u64 {
        self.request(
            "session/prompt",
            json!({"sessionId":session,"prompt":[{"type":"text","text":text}]}),
        )
    }

    /// The agent's text for one prompt, after its ACK.
    fn said(&mut self, session: &str, text: &str) -> String {
        let id = self.prompt(session, text);
        let ack = self.response(id);
        let input = ack["result"]["messageId"]
            .as_str()
            .unwrap_or_else(|| panic!("no ACK: {ack}"))
            .to_owned();
        let message = self.update("agent message", move |u| {
            u["sessionUpdate"] == json!("agent_message")
                && u["_meta"]["oulipoly.ai/parentMessageId"] == json!(input)
        });
        message["content"][0]["text"].as_str().unwrap().to_owned()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn describe_and_policy_report_the_mediated_claude_tools() {
    let f = Fixture::new();
    let described = f.invoke("describe", f.host(), json!({}));
    assert_eq!(
        described["result"]["capabilities"]["tool_mediation_v1"],
        json!(true)
    );
    let plain = f.invoke("describe", json!({"app":"t"}), json!({}));
    assert!(plain["result"]["capabilities"]
        .get("tool_mediation_v1")
        .is_none());
    let evaluate = |launch: Value| {
        f.invoke(
            "policy.evaluate",
            f.host(),
            json!({"settings_id":"claude-primary","mode":"headless",
                "model":{"name":"opus","provider_args":[],"inputs":{"prompt":"hi","named":{}}},
                "launch":launch}),
        )["result"]
            .clone()
    };
    let policy = f.policy(json!({"authority":"trusted-task"}));
    let result = evaluate(json!({"command":"claude","env":{"OULIPOLY_TOOL_MEDIATION_V1":policy}}));
    assert_eq!(result["accepted"], json!(true), "{result}");
    assert_eq!(result["env"]["OULIPOLY_TOOL_MEDIATION_V1"], json!(policy));
    let marker = &result["markers"][0];
    assert_eq!(marker["name"], json!("oulipoly.tool_mediation/v1"));
    agent_provider_contract::tool_mediation::validate("EffectiveMediation", &marker["value"])
        .unwrap();
    assert_eq!(
        marker["value"]["native_tools"],
        json!(["mcp__oulipoly__bash", "Read", "Write", "Edit"])
    );
    // Selected but absent, invalid, or combined with the template's own
    // tool options: refused, never run unrestricted.
    for launch in [
        json!({"command":"claude"}),
        json!({"command":"claude","env":{"OULIPOLY_TOOL_MEDIATION_V1":"{\"protocol\":\"oulipoly.tool_mediation/v2\"}"}}),
        json!({"command":"claude","args":["--allowedTools","Bash"],"env":{"OULIPOLY_TOOL_MEDIATION_V1":policy}}),
        json!({"command":"claude","tool_restrictions":{"claude":{"allowed_tools":["Bash"]}},"env":{"OULIPOLY_TOOL_MEDIATION_V1":policy}}),
    ] {
        let result = evaluate(launch.clone());
        assert_eq!(result["accepted"], json!(false), "{launch} {result}");
        assert!(result["markers"].as_array().unwrap().is_empty());
    }
    // A value spelling an owned option is data, not a conflict.
    let result = evaluate(
        json!({"command":"claude","args":["--append-system-prompt","--tools"],
        "env":{"OULIPOLY_TOOL_MEDIATION_V1":policy}}),
    );
    assert_eq!(result["accepted"], json!(true), "{result}");
}

#[test]
fn prepare_and_one_shot_launch_refuse_what_they_cannot_honour() {
    let f = Fixture::new();
    let policy = f.policy(json!({"allow":["true"]}));
    let refused = |response: Value, code: &str| {
        assert_eq!(response["ok"], json!(false), "{response}");
        assert_eq!(response["error"]["code"], json!(code), "{response}");
    };
    refused(f.prepare(f.template(&[], None)), "tool_mediation_invalid");
    refused(
        f.prepare(f.template(&["--tools", "Bash"], Some(policy.clone()))),
        "tool_mediation_conflict",
    );
    refused(
        f.prepare(f.template(&["--dangerously-skip-permissions"], Some(policy.clone()))),
        "tool_mediation_conflict",
    );
    refused(
        f.prepare(f.template(&[], Some(policy.replace("allow", "deny")))),
        "tool_mediation_invalid",
    );
    let marker = f.path().join("launched");
    let mut host = f.host();
    host["env"]["OULIPOLY_HOST_LAUNCH_OUTPUT_V1"] = json!("1");
    let launch = f.invoke(
        "launch",
        host,
        json!({"settings_id":"claude-primary","mode":"headless",
            "model":{"name":"opus","provider_args":[],"inputs":{"prompt":null,"named":{}}},
            "argv":["/bin/sh","-c",format!("touch {}", marker.display())],
            "working_directory":f.path(),"env":{"OULIPOLY_TOOL_MEDIATION_V1":policy}}),
    );
    refused(launch, "tool_mediation_resident_only");
    assert!(!marker.exists(), "nothing ran");
    assert!(f.calls().is_empty());
}

#[test]
fn mediated_bash_reaches_the_root_ingress_with_the_turn_context() {
    let f = Fixture::new();
    let ingress = Ingress::start(f.path());
    let prepared = f.prepare(f.template(&[], Some(f.policy(json!({"authority":"trusted-task"})))));
    assert_eq!(prepared["ok"], json!(true), "{prepared}");
    let mut client = Client::serve(&prepared["result"], Some(&ingress.path));
    let work = f.path().join("work");
    let session = client.open(&work);
    let said = client.said(&session, "bash git status");
    assert!(
        said.contains("Root v1 work ended: exited with code 0"),
        "{said}"
    );
    assert!(
        said.contains(&format!("ran git status in {}", work.display())),
        "{said}"
    );
    let seen = ingress.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].request["op"], json!("run"));
    assert_eq!(
        seen[0].request["argv"],
        json!(["bash", "-lc", "git status"])
    );
    assert_eq!(seen[0].request["cwd"], json!(work));
    assert!(
        seen[0].ancestry.contains(&(client.child.id() as i32)),
        "the requester runs inside the endpoint's own process tree: {:?}",
        seen[0].ancestry
    );
    let call = f.calls().pop().unwrap();
    assert_eq!(call["tools"], json!(["bash"]));
    assert_eq!(call["servers"], json!(["oulipoly"]));
    assert_eq!(call["opts"]["--tools"], json!("Read,Write,Edit"));
    assert_eq!(
        call["opts"]["--allowedTools"],
        json!("mcp__oulipoly__bash,Read,Write,Edit")
    );
    assert_eq!(call["opts"]["--permission-mode"], json!("dontAsk"));
    assert_eq!(call["opts"]["--setting-sources"], json!(""));
    assert!(call["argv"]
        .as_array()
        .unwrap()
        .contains(&json!("--strict-mcp-config")));
    assert!(call["opts"]["--disallowedTools"]
        .as_str()
        .unwrap()
        .split(',')
        .any(|tool| tool == "Bash"));
    assert_eq!(call["tool_search"], json!("false"));
}

#[test]
fn allow_lists_refuse_in_the_tool_and_nothing_reaches_the_ingress() {
    let f = Fixture::new();
    let ingress = Ingress::start(f.path());
    let prepared = f.prepare(f.template(&[], Some(f.policy(json!({"allow":["make check"]})))));
    let mut client = Client::serve(&prepared["result"], Some(&ingress.path));
    let session = client.open(&f.path().join("work"));
    let said = client.said(&session, "bash make check && curl evil");
    assert!(said.contains("Denied by this root's bash policy"), "{said}");
    assert!(ingress.seen().is_empty());
    let call = f.calls().pop().unwrap();
    assert_eq!(call["opts"]["--tools"], json!(""));
    assert_eq!(call["opts"]["--allowedTools"], json!("mcp__oulipoly__bash"));
    let said = client.said(&session, "bash make check");
    assert!(said.contains("ran make check"), "{said}");
    assert_eq!(ingress.seen().len(), 1);
}

#[test]
fn a_turn_without_the_root_ingress_never_starts_claude() {
    let f = Fixture::new();
    let prepared = f.prepare(f.template(&[], Some(f.policy(json!({"authority":"trusted-task"})))));
    let mut client = Client::serve(&prepared["result"], None);
    let session = client.open(&f.path().join("work"));
    let id = client.prompt(&session, "bash true");
    let response = client.response(id);
    assert!(response["error"].is_object(), "{response}");
    assert!(
        response
            .to_string()
            .contains("tool_mediation_ingress_unavailable")
            || response.to_string().contains("OULIPOLY_ROOT_BASH_V1"),
        "{response}"
    );
    assert!(f.calls().is_empty(), "Claude Code never started");
}

#[test]
fn cancelling_a_turn_ends_the_bridge_and_requester_in_its_group() {
    let f = Fixture::new();
    let ingress = Ingress::start(f.path());
    let prepared = f.prepare(f.template(&[], Some(f.policy(json!({"authority":"trusted-task"})))));
    let mut client = Client::serve(&prepared["result"], Some(&ingress.path));
    let session = client.open(&f.path().join("work"));
    let id = client.prompt(&session, "bash hang");
    client.response(id);
    let deadline = Instant::now() + TIMEOUT;
    while ingress.seen().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the run never reached the ingress"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let requester = ingress.seen()[0].peer;
    let bridge = f.calls().pop().unwrap()["mcp_pid"].as_i64().unwrap();
    assert!(alive(requester.into()) && alive(bridge));
    client.notify("session/cancel", json!({"sessionId":session}));
    let idle = client.update("cancelled idle", |u| u["state"] == json!("idle"));
    assert_eq!(idle["stopReason"], json!("cancelled"), "{idle}");
    while alive(requester.into()) || alive(bridge) || !ingress.seen()[0].eof_while_running {
        assert!(
            Instant::now() < deadline,
            "requester or bridge survived cancel"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn native_inventory_reports_are_observed_and_contradictions_refuse_the_turn() {
    for case in [
        "consistent",
        "extra-tool",
        "missing-tool",
        "extra-server",
        "disconnected",
        "invalid",
        "unreported",
    ] {
        let f = Fixture::new();
        let prepared = f.prepare(f.template(&[], Some(f.policy(json!({"allow":["true"]})))));
        let ingress = Ingress::start(f.path());
        let mut client = Client::serve(&prepared["result"], Some(&ingress.path));
        let session = client.open(&f.path().join("work"));
        if case == "consistent" || case == "unreported" {
            let prompt = if case == "unreported" {
                "no tool".into()
            } else {
                format!("inventory {case}")
            };
            assert_eq!(client.said(&session, &prompt), "no tool");
            client.update("complete idle", |u| {
                u["sessionUpdate"] == json!("state_update") && u["state"] == json!("idle")
            });
        } else {
            let id = client.prompt(&session, &format!("inventory {case}"));
            let response = client.response(id);
            assert!(
                response["error"].is_object()
                    && response
                        .to_string()
                        .contains("native_tool_inventory_mismatch"),
                "{case}: {response}"
            );
        }
        // The SDK retains the actual init observation in the native-turn journal;
        // arbitrary markers are not fabricated as agent conversation messages.
        let mut journals = Vec::new();
        fn collect(path: &Path, output: &mut Vec<String>) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(&path, output);
                } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                    output.push(std::fs::read_to_string(path).unwrap());
                }
            }
        }
        collect(&f.path().join("data"), &mut journals);
        let observation: Value = journals
            .iter()
            .flat_map(|s| s.lines())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|event| event["name"] == json!("claude.native_tool_inventory"))
            .unwrap()["value"]
            .clone();
        assert_eq!(observation["enforcement_attested"], json!(false));
        let expected = match case {
            "unreported" => "not-reported",
            "consistent" | "extra-server" | "disconnected" => "consistent",
            "invalid" => "invalid",
            _ => "contradictory",
        };
        assert_eq!(
            observation["tools_observation"],
            json!(expected),
            "{case}: {observation}"
        );
        assert!(ingress.seen().is_empty());
    }
}

fn marker(result: &Value, name: &str) -> Option<Value> {
    result["markers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == json!(name))
        .map(|m| m["value"].clone())
}

#[test]
fn exploration_is_advertised_only_to_a_host_that_selects_it() {
    let f = Fixture::new();
    let selected = f.invoke("describe", f.exploring_host(), json!({}));
    assert_eq!(
        selected["result"]["capabilities"]["exploration_v1"],
        json!(true),
        "{selected}"
    );
    for host in [f.host(), json!({"app":"t"})] {
        let described = f.invoke("describe", host, json!({}));
        assert!(
            described["result"]["capabilities"]
                .get("exploration_v1")
                .is_none(),
            "{described}"
        );
    }
}

#[test]
fn policy_reports_one_explore_tool_only_for_an_admitted_offer() {
    let f = Fixture::new();
    let policy = f.policy(json!({"authority":"trusted-task"}));
    let offer = f.offer(&["luna-max", "terra"]);
    let result = f.evaluate(
        f.exploring_host(),
        json!({"OULIPOLY_TOOL_MEDIATION_V1":policy,"OULIPOLY_EXPLORATION_V1":offer}),
    );
    assert_eq!(result["accepted"], json!(true), "{result}");
    assert_eq!(result["env"]["OULIPOLY_EXPLORATION_V1"], json!(offer));
    let effective = marker(&result, "oulipoly.exploration/v1").expect("exploration marker");
    agent_provider_contract::exploration::validate("EffectiveExploration", &effective).unwrap();
    assert_eq!(effective["routes"], json!(["luna-max", "terra"]));
    assert_eq!(effective["ingress_env"], json!(INGRESS_ENV));
    let mediation = marker(&result, "oulipoly.tool_mediation/v1").expect("mediation marker");
    let tools: Vec<&Value> = mediation["native_tools"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    // trusted-task's file tools, the mediated command tool, and exactly one
    // more: the exploration tool the exploration marker names.
    assert_eq!(tools.len(), 5, "{mediation}");
    assert!(tools.contains(&&mediation["tool"]));
    assert!(
        tools.contains(&&effective["tool"]),
        "{mediation} {effective}"
    );
    for builtin in ["Read", "Write", "Edit"] {
        assert!(tools.contains(&&json!(builtin)), "{mediation}");
    }

    let result = f.evaluate(
        f.exploring_host(),
        json!({"OULIPOLY_TOOL_MEDIATION_V1":policy}),
    );
    assert_eq!(result["accepted"], json!(true), "{result}");
    assert!(marker(&result, "oulipoly.exploration/v1").is_none());
    assert_eq!(
        marker(&result, "oulipoly.tool_mediation/v1").unwrap()["native_tools"],
        json!(["mcp__oulipoly__bash", "Read", "Write", "Edit"])
    );

    let mut unmediated_host = f.exploring_host();
    unmediated_host["env"]
        .as_object_mut()
        .unwrap()
        .remove("OULIPOLY_HOST_TOOL_MEDIATION_V1");
    let mut unknown_field: Value = serde_json::from_str(&offer).unwrap();
    unknown_field["model"] = json!("opus");
    for (host, env) in [
        (
            f.host(),
            json!({"OULIPOLY_TOOL_MEDIATION_V1":policy,"OULIPOLY_EXPLORATION_V1":offer}),
        ),
        (unmediated_host, json!({"OULIPOLY_EXPLORATION_V1":offer})),
        (
            f.exploring_host(),
            json!({"OULIPOLY_TOOL_MEDIATION_V1":policy,"OULIPOLY_EXPLORATION_V1":unknown_field.to_string()}),
        ),
        (
            f.exploring_host(),
            json!({"OULIPOLY_TOOL_MEDIATION_V1":policy,"OULIPOLY_EXPLORATION_V1":"[]"}),
        ),
    ] {
        let result = f.evaluate(host.clone(), env.clone());
        assert_eq!(result["accepted"], json!(false), "{env} {result}");
        assert!(marker(&result, "oulipoly.exploration/v1").is_none());
        assert!(
            result["diagnostics"]
                .to_string()
                .to_lowercase()
                .contains("exploration"),
            "refused for the offer: {result}"
        );
        let mut template = f.template(&[], None);
        template["env"] = env.clone();
        let response = f.prepare_for(host, template);
        assert_eq!(response["ok"], json!(false), "{env} {response}");
        assert!(
            response["error"]
                .to_string()
                .to_lowercase()
                .contains("exploration"),
            "{response}"
        );
    }
    // A one-shot launch is never mediated, so an offer is refused there too.
    let launched = f.path().join("launched");
    let mut host = f.exploring_host();
    host["env"]["OULIPOLY_HOST_LAUNCH_OUTPUT_V1"] = json!("1");
    host["env"]
        .as_object_mut()
        .unwrap()
        .remove("OULIPOLY_HOST_TOOL_MEDIATION_V1");
    let launch = f.invoke(
        "launch",
        host,
        json!({"settings_id":"claude-primary","mode":"headless",
            "model":{"name":"opus","provider_args":[],"inputs":{"prompt":null,"named":{}}},
            "argv":["/bin/sh","-c",format!("touch {}", launched.display())],
            "working_directory":f.path(),"env":{"OULIPOLY_EXPLORATION_V1":offer}}),
    );
    assert_eq!(launch["ok"], json!(false), "{launch}");
    assert!(!launched.exists(), "nothing ran");
    assert!(f.calls().is_empty());
}

#[test]
fn an_offered_parent_asks_the_owner_through_one_explore_tool_and_keeps_bash() {
    let f = Fixture::new();
    let ingress = Ingress::start(f.path());
    let prepared = f.prepare_for(f.exploring_host(), f.offered(Some(f.offer(&["luna-max"]))));
    assert_eq!(prepared["ok"], json!(true), "{prepared}");
    let mut client = Client::serve(&prepared["result"], Some(&ingress.path));
    let work = f.path().join("work");
    let session = client.open(&work);
    let said = client.said(&session, "explore - where is the bridge wired?");
    assert!(
        said.contains("Explorer child-1 (route luna-max): answered."),
        "{said}"
    );
    assert!(
        said.contains("answer to where is the bridge wired?"),
        "{said}"
    );
    let call = f.calls().pop().unwrap();
    assert_eq!(call["tools"], json!(["bash", "explore"]));
    assert_eq!(call["servers"], json!(["oulipoly"]));
    assert_eq!(call["offer_in_mcp_config"], json!(true));
    let allowed: Vec<&str> = call["opts"]["--allowedTools"]
        .as_str()
        .unwrap()
        .split(',')
        .collect();
    assert_eq!(allowed, ["mcp__oulipoly__bash", "mcp__oulipoly__explore"]);
    assert_eq!(call["opts"]["--tools"], json!(""));
    let seen = ingress.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].request["op"], json!("child"));
    assert_eq!(seen[0].request["route"], json!("luna-max"));
    assert!(
        seen[0].ancestry.contains(&(client.child.id() as i32)),
        "the child requester runs inside the endpoint's own process tree: {:?}",
        seen[0].ancestry
    );
    let said = client.said(&session, "explore terra anything");
    assert!(said.contains("Nothing was asked"), "{said}");
    assert_eq!(ingress.seen().len(), 1);
    let said = client.said(&session, "bash true");
    assert!(said.contains("ran true"), "{said}");
    assert_eq!(ingress.seen().len(), 2);
    // A native report that leaves out the offered tool contradicts it.
    let id = client.prompt(&session, "inventory no-explore");
    let response = client.response(id);
    assert!(
        response
            .to_string()
            .contains("native_tool_inventory_mismatch"),
        "{response}"
    );
}

#[test]
fn an_offer_that_was_not_admitted_never_reaches_claude_or_its_bridge() {
    let f = Fixture::new();
    let ingress = Ingress::start(f.path());
    let offer = f.offer(&["luna-max"]);
    let prepared = f.prepare_for(f.exploring_host(), f.offered(None));
    assert_eq!(prepared["ok"], json!(true), "{prepared}");
    let mut client = Client::serve_with(
        &prepared["result"],
        Some(&ingress.path),
        &[("OULIPOLY_EXPLORATION_V1", offer.as_str())],
    );
    let session = client.open(&f.path().join("work"));
    let said = client.said(&session, "explore - where?");
    assert_eq!(said, "explore is not an allowed tool");
    let call = f.calls().pop().unwrap();
    assert_eq!(call["offer_in_mcp_config"], json!(false), "{call}");
    assert_eq!(call["offer_in_native_env"], json!(false), "{call}");
    let said = client.said(&session, "bash true");
    assert!(said.contains("ran true"), "{said}");
    let call = f.calls().pop().unwrap();
    assert_eq!(call["tools"], json!(["bash"]), "{call}");
    assert!(ingress
        .seen()
        .iter()
        .all(|seen| seen.request["op"] != json!("child")));
}

#[test]
fn cancelling_an_explore_turn_ends_its_requester_and_bridge() {
    let f = Fixture::new();
    let ingress = Ingress::start(f.path());
    let prepared = f.prepare_for(f.exploring_host(), f.offered(Some(f.offer(&["luna-max"]))));
    let mut client = Client::serve(&prepared["result"], Some(&ingress.path));
    let session = client.open(&f.path().join("work"));
    let id = client.prompt(&session, "explore luna-max hang");
    client.response(id);
    let deadline = Instant::now() + TIMEOUT;
    while ingress.seen().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the request never reached the ingress"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let requester = ingress.seen()[0].peer;
    let bridge = f.calls().pop().unwrap()["mcp_pid"].as_i64().unwrap();
    assert!(alive(requester.into()) && alive(bridge));
    client.notify("session/cancel", json!({"sessionId":session}));
    let idle = client.update("cancelled idle", |u| u["state"] == json!("idle"));
    assert_eq!(idle["stopReason"], json!("cancelled"), "{idle}");
    while alive(requester.into()) || alive(bridge) || !ingress.seen()[0].eof_while_running {
        assert!(
            Instant::now() < deadline,
            "requester or bridge survived cancel"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
