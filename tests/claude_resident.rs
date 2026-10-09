//! Host-selected extensions through the real provider binary:
//! `oulipoly.launch_output/v1` custody on every launch exit path, and resident
//! sessions (`resident.prepare`, `resident.serve`) over a fake Claude Code CLI
//! that speaks print-mode stream JSON.
#![cfg(target_os = "linux")]

use agent_provider_contract::resident_session as extension;
use agent_provider_contract::SchemaRegistry;
use agent_provider_execution::encoding::{decode_base64, sha256_hex};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const CONTRACT: &str = agent_provider_contract::CONTRACT_VERSION;
const TIMEOUT: Duration = Duration::from_secs(30);

/// Fake `claude -p --input-format stream-json --output-format stream-json`.
const FAKE_CLAUDE: &str = r#"#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
line = sys.stdin.readline()
with open(os.environ['CALLS'], 'a') as f:
    f.write(json.dumps({'argv': args, 'stdin': line}) + '\n')
message = json.loads(line)
prompt = message['message']['content'][0]['text']
# Native option arity matters: admitted text can itself spell --session-id.
options = {}
i = 0
while i < len(args):
    flag = args[i]
    if flag in ['--append-system-prompt', '--model', '--input-format', '--output-format', '--resume', '--session-id']:
        options[flag] = args[i+1]
        i += 2
    else:
        i += 1
session = options.get('--resume', options.get('--session-id', 'chosen-by-claude'))
def emit(event):
    print(json.dumps(event), flush=True)
words = prompt.split()
# These programs really started and received input; neither 126 nor an exact
# echo of a provider-chosen candidate proves an observed native session.
if words[0] == 'preinitfail':
    print('fixture failed before system/init (not an exec proof)', file=sys.stderr, flush=True)
    sys.exit(126)
if words[0] == 'preinitecho':
    emit({'type': 'user', 'uuid': message['uuid'], 'session_id': session, 'message': message['message']})
    sys.exit(1)
if words[0] == 'invalidsession':
    session = 'not-a-uuid'
if words[0] == 'driftsession':
    session = '11111111-1111-4111-8111-111111111111'
emit({'type': 'system', 'subtype': 'init', 'session_id': session, 'model': 'fixture'})
if words[0] == 'noconsume':
    sys.exit(4)
echo = {'type': 'user', 'uuid': message['uuid'], 'session_id': session, 'message': message['message']}
if words[0] == 'foreignreplay':
    echo['uuid'] = '22222222-2222-4222-8222-222222222222'
    echo['isReplay'] = True
if words[0] == 'uuidwithouthyphens':
    echo['uuid'] = echo['uuid'].replace('-', '')
if words[0] == 'foreignsession':
    echo['session_id'] = '22222222-2222-4222-8222-222222222222'
if words[0] == 'foreigncontent':
    echo['message'] = {'role':'user','content':[{'type':'text','text':'unrelated'}]}
if words[0] == 'toolreplay':
    echo['parent_tool_use_id'] = 'toolu_parent'
emit(echo)
if words[0] == 'hang':
    child = subprocess.Popen(['sleep', '300'])
    open(os.path.join(words[1], 'descendant.pid'), 'w').write(str(child.pid))
    emit({'type': 'assistant', 'parent_tool_use_id': None, 'message': {'content': [{'type': 'text', 'text': 'waiting'}]}})
    child.wait()
    sys.exit(0)
emit({'type':'assistant','isSidechain':True,'parent_tool_use_id':None,'message':{'content':[{'type':'text','text':'sidechain text'}]}})
emit({'type': 'assistant', 'parent_tool_use_id': 'toolu_1', 'message': {'content': [{'type': 'text', 'text': 'subagent text'}]}})
emit({'type': 'assistant', 'parent_tool_use_id': None, 'message': {'content': [{'type': 'tool_use', 'id': 'toolu_2', 'name': 'Read', 'input': {}}]}})
if words[0] == 'fail':
    emit({'type': 'result', 'subtype': 'error_during_execution', 'is_error': True, 'session_id': session})
    sys.exit(1)
emit({'type': 'assistant', 'parent_tool_use_id': None, 'message': {'content': [{'type': 'text', 'text': 'reply to %s on %s' % (prompt, session)}]}})
if words[0] == 'noresult':
    sys.exit(0)
emit({'type': 'result', 'subtype': 'success', 'is_error': False, 'stop_reason': 'end_turn', 'session_id': session})
"#;

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("u92-correction-claude-resident-")
            .tempdir_in("/tmp")
            .unwrap();
        let claude = root.path().join("claude");
        std::fs::write(&claude, FAKE_CLAUDE).unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(root.path().join("work")).unwrap();
        Self { root }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn host(&self, env: Value) -> Value {
        let mut env = env;
        env["CALLS"] = json!(self.path().join("calls.jsonl"));
        json!({"app":"oulipoly-agent-runner","working_directory":self.path(),
            "data_root":self.path().join("data"),"env":env})
    }

    fn calls(&self) -> Vec<Value> {
        std::fs::read_to_string(self.path().join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn invoke(&self, operation: &str, host: Value, params: Value) -> (i32, String) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let request = json!({"contract":CONTRACT,"request_id":format!("req-{operation}-{n}"),
            "provider_instance_id":"claude-primary","host":host,"params":params});
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"))
            .arg(operation)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8(output.stdout).unwrap(),
        )
    }

    fn launch_params(&self, argv: Value) -> Value {
        json!({"settings_id":"claude-primary","mode":"headless",
            "model":{"name":"claude-sonnet","provider_args":[],"inputs":{"prompt":null,"named":{}}},
            "argv":argv,"working_directory":self.path(),"env":{},
            "output_delivery":{"protocol":"oulipoly.launch_output/v1"}})
    }

    fn template(&self) -> Value {
        json!({"settings_id":"claude-primary","mode":"headless",
            "model":{"name":"claude-opus","provider_args":["--model","opus"],"inputs":{"prompt":null,"named":{}}},
            "argv":[self.path().join("claude"),"-p","--output-format","json","--model","opus"],
            "env":{"CALLS":self.path().join("calls.jsonl")}})
    }

    fn prepare(&self) -> Value {
        let (code, out) = self.invoke(
            "resident.prepare",
            self.host(json!({"OULIPOLY_HOST_RESIDENT_SESSION_V1":"1"})),
            json!({"protocol":"oulipoly.resident_session/v1","launch":self.template()}),
        );
        let response: Value = serde_json::from_str(&out).unwrap();
        assert_eq!((code, &response["ok"]), (0, &json!(true)), "{response}");
        response["result"].clone()
    }
}

/// Runner's spool rule: the completion marker equals every data event so far
/// and is the last event before `exit`.
fn assert_complete_output(events: &[Value]) {
    let registry = SchemaRegistry::new();
    let exit = events
        .iter()
        .position(|e| e["kind"] == json!("exit"))
        .expect("exit event");
    assert_eq!(exit, events.len() - 1);
    assert!(exit >= 1, "no completion marker before exit: {events:?}");
    let marker = &events[exit - 1];
    assert_eq!(
        marker["name"],
        json!("oulipoly.launch_output_complete/v1"),
        "{events:?}"
    );
    registry.validate_launch_event("marker", marker).unwrap();
    let channel = |kind: &str| {
        let bytes: Vec<u8> = events[..exit - 1]
            .iter()
            .filter(|e| e["kind"] == json!(kind))
            .flat_map(|e| decode_base64(e["data_base64"].as_str().unwrap()).unwrap())
            .collect();
        json!({"bytes":bytes.len(),"sha256":sha256_hex(&bytes)})
    };
    let value = &marker["value"];
    assert_eq!(value["stdout"], channel("stdout"));
    assert_eq!(value["stderr"], channel("stderr"));
    let count = events[..exit - 1]
        .iter()
        .filter(|e| matches!(e["kind"].as_str(), Some("stdout" | "stderr")))
        .count();
    assert_eq!(value["data_event_count"], json!(count));
}

fn events(out: &str) -> Vec<Value> {
    out.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn describe_advertises_selected_extensions_only() {
    let f = Fixture::new();
    let registry = SchemaRegistry::new();
    let (_, plain) = f.invoke("describe", f.host(json!({})), json!({}));
    let plain: Value = serde_json::from_str(&plain).unwrap();
    registry.validate_response("describe", &plain).unwrap();
    let capabilities = plain["result"]["capabilities"].as_object().unwrap();
    assert!(!capabilities.contains_key("launch_output_v1"));
    assert!(!capabilities.contains_key("resident_session_v1"));
    let (_, selected) = f.invoke(
        "describe",
        f.host(
            json!({"OULIPOLY_HOST_LAUNCH_OUTPUT_V1":"1","OULIPOLY_HOST_RESIDENT_SESSION_V1":"1",
            "OULIPOLY_HOST_RESIDENT_SESSION_V2":"1"}),
        ),
        json!({}),
    );
    let selected: Value = serde_json::from_str(&selected).unwrap();
    registry.validate_response("describe", &selected).unwrap();
    let capabilities = selected["result"]["capabilities"].as_object().unwrap();
    assert_eq!(capabilities["launch_output_v1"], json!(true));
    assert_eq!(capabilities["resident_session_v1"], json!(true));
    assert!(!capabilities.contains_key("resident_session_v2"));
    assert_eq!(extension::select(&[1], capabilities), Ok(1));
    let mut future = selected.clone();
    future["result"]["contract_versions"] = json!(["oulipoly.provider/v2", "oulipoly.provider/v1"]);
    future["result"]["preferred_contract"] = json!("oulipoly.provider/v2");
    future["result"]["capabilities"]["resident_session_v2"] = json!(true);
    future["result"]["capabilities"]["future_capability"] = json!({"new_shape":42});
    let admitted = registry
        .decode_response::<agent_provider_contract::operations::Describe>(
            &serde_json::to_vec(&future).unwrap(),
        )
        .unwrap();
    let advertised = &admitted.value().result;
    assert_eq!(
        agent_provider_contract::negotiation::select_contract_version(
            &["oulipoly.provider/v1"],
            &advertised.contract_versions,
            &advertised.preferred_contract
        ),
        Ok("oulipoly.provider/v1".into())
    );
    let caps = serde_json::to_value(&advertised.capabilities).unwrap();
    assert_eq!(extension::select(&[1], caps.as_object().unwrap()), Ok(1));
    future["result"]["capabilities"]["resident_session_v1"] = json!("true");
    assert!(registry
        .decode_response::<agent_provider_contract::operations::Describe>(
            &serde_json::to_vec(&future).unwrap()
        )
        .is_err());
    future["result"]["capabilities"]["resident_session_v1"] = json!(true);
    future["result"]["preferred_contract"] = json!("oulipoly.provider/v3");
    assert!(registry
        .decode_response::<agent_provider_contract::operations::Describe>(
            &serde_json::to_vec(&future).unwrap()
        )
        .is_err());
}

#[test]
fn launch_output_custody_holds_on_every_exit_path() {
    let f = Fixture::new();
    let selected = f.host(json!({"OULIPOLY_HOST_LAUNCH_OUTPUT_V1":"1"}));
    // Native path, including interleaved stderr and a trailing partial line.
    let (code, out) = f.invoke(
        "launch",
        selected.clone(),
        f.launch_params(json!([
            "/bin/sh",
            "-c",
            "printf 'one\\n'; printf 'err' >&2; printf 'two'"
        ])),
    );
    assert_eq!(code, 0, "{out}");
    let native = events(&out);
    assert_complete_output(&native);
    assert!(native.iter().any(|e| e["kind"] == json!("stdout")));
    // Observed start failure.
    let mut start = f.launch_params(json!(["/nonexistent/claude"]));
    start["argv"] = json!(["/nonexistent/claude"]);
    let (_, out) = f.invoke("launch", selected.clone(), start);
    let failed = events(&out);
    assert_eq!(
        failed.last().unwrap()["status"]["kind"],
        json!("spawn_error")
    );
    assert_complete_output(&failed);
    // Settled without a native process.
    let (_, out) = f.invoke("launch", selected.clone(), f.launch_params(json!([])));
    let settled = events(&out);
    assert_eq!(
        settled.last().unwrap()["status"]["kind"],
        json!("spawn_error")
    );
    assert_complete_output(&settled);
    // Unselected: refused before any native effect, never silently ignored.
    let marker = f.path().join("ran");
    let (code, out) = f.invoke(
        "launch",
        f.host(json!({})),
        f.launch_params(json!([
            "/bin/sh",
            "-c",
            format!("touch {}", marker.display())
        ])),
    );
    assert_ne!(code, 0);
    assert!(out.contains("launch_output_not_selected"), "{out}");
    assert!(!marker.exists());
    // Without output_delivery a launch keeps its earlier events: no marker.
    let mut legacy = f.launch_params(json!(["/bin/sh", "-c", "echo legacy"]));
    legacy.as_object_mut().unwrap().remove("output_delivery");
    let (_, out) = f.invoke("launch", selected, legacy);
    assert!(!out.contains("launch_output_complete"), "{out}");
}

#[test]
fn prepare_records_a_resident_template_and_refuses_session_flags() {
    let f = Fixture::new();
    let prepared = f.prepare();
    let result = extension::decode_prepare_result(&prepared).unwrap();
    assert_eq!(result.invocation.args[..2], ["resident.serve", "--config"]);
    let bytes = std::fs::read(&result.invocation.args[2]).unwrap();
    assert_eq!(sha256_hex(&bytes), result.config_sha256);
    let (_, out) = f.invoke(
        "resident.prepare",
        f.host(json!({})),
        json!({"protocol":"oulipoly.resident_session/v1","launch":f.template()}),
    );
    assert!(out.contains("resident_session_not_selected"), "{out}");
    let mut template = f.template();
    template["argv"] = json!(["claude", "-p", "--resume", "someone-else"]);
    let (_, out) = f.invoke(
        "resident.prepare",
        f.host(json!({"OULIPOLY_HOST_RESIDENT_SESSION_V1":"1"})),
        json!({"protocol":"oulipoly.resident_session/v1","launch":template}),
    );
    assert!(out.contains("invalid_resident_argv"), "{out}");
    assert!(f.calls().is_empty(), "prepare runs no native command");
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<Value>,
    seen: Vec<Value>,
    next_id: u64,
}

impl Client {
    fn serve(prepared: &Value) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"));
        for arg in prepared["invocation"]["args"].as_array().unwrap() {
            command.arg(arg.as_str().unwrap());
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

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
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

    fn idle_for(&mut self, id: &str) -> Value {
        let id = id.to_owned();
        let idle = self.update("tagged idle", move |u| {
            u["state"] == json!("idle") && u["_meta"]["oulipoly.ai/lastUserMessageId"] == json!(id)
        });
        extension::validate("TurnStopReason", &idle["stopReason"]).unwrap();
        extension::validate("NativeTurnMeta", &idle["_meta"]["oulipoly.ai/nativeTurn"]).unwrap();
        idle
    }

    fn prompt(&mut self, session: &str, text: &str, key: Option<&str>) -> u64 {
        let mut params = json!({"sessionId":session,"prompt":[{"type":"text","text":text}]});
        if let Some(key) = key {
            params["_meta"] = json!({"oulipoly.ai/messageKey": key});
        }
        self.request("session/prompt", params)
    }

    fn open(&mut self, cwd: &Path) -> String {
        let init = self.call(
            "initialize",
            json!({"protocolVersion":2,"info":{"name":"t","version":"0"}}),
        );
        assert_eq!(init["result"]["info"]["name"], json!("agent-runner-claude"));
        self.call("session/new", json!({"cwd":cwd}))["result"]["sessionId"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn message_id(response: &Value) -> String {
    response["result"]["messageId"]
        .as_str()
        .unwrap_or_else(|| panic!("no messageId in {response}"))
        .to_owned()
}

fn argv(call: &Value) -> Vec<String> {
    serde_json::from_value(call["argv"].clone()).unwrap()
}

fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == flag)
        .map(|at| argv[at + 1].clone())
}

#[test]
fn resident_turns_create_then_resume_one_native_session() {
    let f = Fixture::new();
    let prepared = f.prepare();
    let mut client = Client::serve(&prepared);
    let session = client.open(&f.path().join("work"));
    let first = client.prompt(&session, "hello", Some("k1"));
    let first = message_id(&client.response(first));
    let text = client.update("agent message", |u| {
        u["sessionUpdate"] == json!("agent_message")
    });
    assert_eq!(text["_meta"]["oulipoly.ai/parentMessageId"], json!(first));
    let said = text["content"][0]["text"].as_str().unwrap().to_owned();
    let native = said.rsplit(' ').next().unwrap().to_owned();
    assert_eq!(
        said,
        format!("reply to hello on {native}"),
        "subagent and tool-use records are not agent messages"
    );
    let idle = client.idle_for(&first);
    assert_eq!(idle["stopReason"], json!("end_turn"));
    assert_eq!(
        idle["_meta"]["oulipoly.ai/nativeTurn"]["launch_output"]["data_event_count"],
        json!(1)
    );

    let second = client.prompt(&session, "again", None);
    let second = message_id(&client.response(second));
    let text = client.update("second message", |u| {
        u["sessionUpdate"] == json!("agent_message")
    });
    assert_eq!(
        text["content"][0]["text"],
        json!(format!("reply to again on {native}"))
    );
    client.idle_for(&second);

    let duplicate = client.prompt(&session, "hello", Some("k1"));
    let duplicate = client.response(duplicate);
    assert_eq!(message_id(&duplicate), first);
    assert_eq!(
        duplicate["result"]["_meta"]["oulipoly.ai/duplicate"],
        json!(true)
    );

    let calls = f.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let (one, two) = (argv(&calls[0]), argv(&calls[1]));
    assert_eq!(flag_value(&one, "--session-id"), Some(native.clone()));
    assert_eq!(flag_value(&one, "--resume"), None);
    assert_eq!(flag_value(&two, "--resume"), Some(native));
    for call in [&one, &two] {
        assert_eq!(
            flag_value(call, "--output-format").as_deref(),
            Some("stream-json")
        );
        assert_eq!(
            flag_value(call, "--input-format").as_deref(),
            Some("stream-json")
        );
        assert!(call.contains(&"--replay-user-messages".to_owned()));
        assert_eq!(call.iter().filter(|a| *a == "--output-format").count(), 1);
        assert_eq!(flag_value(call, "--model").as_deref(), Some("opus"));
    }
    let stdin: Value = serde_json::from_str(calls[0]["stdin"].as_str().unwrap()).unwrap();
    assert_eq!(stdin["type"], json!("user"));
    assert_eq!(stdin["message"]["content"][0]["text"], json!("hello"));
}

#[test]
fn unconsumed_failed_and_cancelled_turns_keep_their_meaning() {
    let f = Fixture::new();
    let prepared = f.prepare();
    let mut client = Client::serve(&prepared);
    let session = client.open(&f.path().join("work"));
    let refused = client.prompt(&session, "noconsume", None);
    assert_eq!(client.response(refused)["error"]["code"], json!(-32010));

    let failed = client.prompt(&session, "fail", None);
    let failed = message_id(&client.response(failed));
    assert_eq!(
        client.idle_for(&failed)["stopReason"],
        json!("_oulipoly_native_failed")
    );

    let silent = client.prompt(&session, "noresult", None);
    let silent = message_id(&client.response(silent));
    let idle = client.idle_for(&silent);
    assert_eq!(
        idle["stopReason"],
        json!("_oulipoly_native_failed"),
        "exit 0 without a result"
    );
    assert_eq!(
        idle["_meta"]["oulipoly.ai/nativeTurn"]["status"],
        json!({"kind":"exited","code":1})
    );

    let marks = f.path().join("marks");
    std::fs::create_dir(&marks).unwrap();
    let hang = client.prompt(&session, &format!("hang {}", marks.display()), None);
    let hang = message_id(&client.response(hang));
    let pid_path: PathBuf = marks.join("descendant.pid");
    let start = Instant::now();
    let descendant: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_path)
            .ok()
            .and_then(|t| t.trim().parse().ok())
        {
            break pid;
        }
        assert!(start.elapsed() < TIMEOUT);
        std::thread::sleep(Duration::from_millis(20));
    };
    client.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}));
    assert_eq!(client.idle_for(&hang)["stopReason"], json!("cancelled"));
    let start = Instant::now();
    while std::fs::read_to_string(format!("/proc/{descendant}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit_once(") ")
            .is_some_and(|(_, r)| r.starts_with('Z'))
    }) {
        assert!(start.elapsed() < TIMEOUT, "descendant survived cancel");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn policy_values_spelling_resident_flags_are_preserved() {
    let f = Fixture::new();
    for value in [
        "ordinary text",
        "--resume",
        "-p",
        "--output-format",
        "--session-id",
    ] {
        let model = f.template()["model"].clone();
        let (code, policy) = f.invoke("policy.evaluate", f.host(json!({})), json!({
            "settings_id":"test", "mode":"headless", "model":model,
            "launch":{"command":f.path().join("claude"),"prompt_mode":"stdin","system_prompt_override":value,"env":{"CALLS":f.path().join("calls.jsonl")}}}));
        let policy: Value = serde_json::from_str(&policy).unwrap();
        assert_eq!(code, 0, "{policy}");
        assert_eq!(policy["result"]["accepted"], json!(true));
        let (code, prepared) = f.invoke(
            "resident.prepare",
            f.host(json!({"OULIPOLY_HOST_RESIDENT_SESSION_V1":"1"})),
            json!({"protocol":"oulipoly.resident_session/v1","launch":{
                "settings_id":"test","mode":"headless","model":model,
                "argv":policy["result"]["argv"],"env":policy["result"]["env"]}}),
        );
        let prepared: Value = serde_json::from_str(&prepared).unwrap();
        assert_eq!(code, 0, "{prepared}");
        let mut client = Client::serve(&prepared["result"]);
        let session = client.open(&f.path().join("work"));
        let req = client.prompt(&session, "hello", None);
        let id = message_id(&client.response(req));
        client.idle_for(&id);
        let calls = f.calls();
        let args = argv(calls.last().unwrap());
        assert_eq!(
            flag_value(&args, "--append-system-prompt").as_deref(),
            Some(value)
        );
        assert_eq!(
            args.iter().filter(|a| *a == "-p").count(),
            if value == "-p" { 2 } else { 1 }
        );
    }
}

#[test]
fn consumption_requires_this_input_and_session_identity() {
    let f = Fixture::new();
    let prepared = f.prepare();
    let mut client = Client::serve(&prepared);
    let session = client.open(&f.path().join("work"));
    for prompt in [
        "foreignreplay",
        "uuidwithouthyphens",
        "foreignsession",
        "foreigncontent",
        "toolreplay",
    ] {
        let req = client.prompt(&session, prompt, None);
        let response = client.response(req);
        assert_eq!(
            response["error"]["code"],
            json!(-32010),
            "{prompt}: {response}"
        );
    }
    let req = client.prompt(&session, "hello", None);
    let id = message_id(&client.response(req));
    assert_eq!(client.idle_for(&id)["stopReason"], json!("end_turn"));
}

#[test]
fn native_session_shape_and_drift_are_refused_without_consumption() {
    for prompt in ["invalidsession", "driftsession"] {
        let f = Fixture::new();
        let prepared = f.prepare();
        let mut client = Client::serve(&prepared);
        let session = client.open(&f.path().join("work"));
        let first = client.prompt(&session, "hello", None);
        let id = message_id(&client.response(first));
        client.idle_for(&id);
        let known = flag_value(&argv(&f.calls()[0]), "--session-id").unwrap();
        let req = client.prompt(&session, prompt, None);
        let response = client.response(req);
        assert_eq!(response["error"]["code"], json!(-32011), "{response}");
        let config_path = PathBuf::from(prepared["invocation"]["args"][2].as_str().unwrap());
        let record_path = config_path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("sessions")
            .join(&session)
            .join("session.json");
        let record: Value = serde_json::from_slice(&std::fs::read(record_path).unwrap()).unwrap();
        assert_eq!(record["native_session_id"], json!(known));
        let next = client.prompt(&session, "hello", None);
        let next = message_id(&client.response(next));
        client.idle_for(&next);
        assert_eq!(
            flag_value(&argv(f.calls().last().unwrap()), "--resume"),
            Some(known)
        );
    }
}

fn session_directory(prepared: &Value, session: &str) -> PathBuf {
    PathBuf::from(prepared["invocation"]["args"][2].as_str().unwrap())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("sessions")
        .join(session)
}

fn session_record(prepared: &Value, session: &str) -> Value {
    serde_json::from_slice(
        &std::fs::read(session_directory(prepared, session).join("session.json")).unwrap(),
    )
    .unwrap()
}

fn turn_events(prepared: &Value, session: &str, native: &Value) -> Vec<Value> {
    let key = agent_provider_execution::custody::request_key(
        None,
        native["request_id"].as_str().unwrap(),
    );
    let journal = session_directory(prepared, session)
        .join("turns")
        .join(format!("{key}.jsonl"));
    events(&std::fs::read_to_string(journal).unwrap())
}

fn assert_no_reported_identity(events: &[Value]) {
    assert_complete_output(events);
    assert!(
        !events.iter().any(|event| event["name"]
            == json!(agent_provider_execution::resident::PROVIDER_SESSION_MARKER)
            || event["name"]
                == json!(agent_provider_execution::resident::SUBMITTED_USER_TURN_MARKER)),
        "{events:?}"
    );
    assert!(
        events.last().unwrap().get("session").is_none(),
        "candidate must not appear in the exit as known identity: {events:?}"
    );
}

/// Intent: U277 G3 / ROOT D3, U282 D5 and the U283 goal. An executed
/// pre-init failure is not a never-started fact; the candidate must remain
/// unobserved and later work must stay blocked across endpoint reopen.
#[test]
fn pre_identity_failures_block_later_work_and_reopen_without_rerun() {
    for prompt in ["preinitfail", "preinitecho"] {
        let f = Fixture::new();
        let prepared = f.prepare();
        let mut client = Client::serve(&prepared);
        let session = client.open(&f.path().join("work"));
        let request = client.prompt(&session, prompt, Some("original"));
        let response = client.response(request);
        assert_eq!(
            response["error"]["code"],
            json!(-32010),
            "{prompt}: {response}"
        );
        let native = &response["error"]["data"]["nativeTurn"];
        assert_eq!(native["custody"], json!("complete"), "{response}");
        assert_eq!(
            native["status"],
            json!({"kind":"exited","code":if prompt == "preinitfail" {126} else {1}})
        );
        let journal = turn_events(&prepared, &session, native);
        assert_no_reported_identity(&journal);
        eprintln!("control={prompt} journal={}", json!(journal));
        let record = session_record(&prepared, &session);
        assert!(
            record["native_session_id"].is_null(),
            "candidate is not observed: {record}"
        );
        assert!(record["create_native_session_id"].is_string(), "{record}");
        assert!(record["native_session_uncertain"].is_string(), "{record}");
        let next = client.prompt(&session, "hello", None);
        assert_eq!(client.response(next)["error"]["code"], json!(-32012));
        drop(client);

        let mut client = Client::serve(&prepared);
        client.call(
            "initialize",
            json!({"protocolVersion":2,"info":{"name":"t","version":"0"}}),
        );
        let loaded = client.call(
            "session/resume",
            json!({"sessionId":session,"cwd":f.path().join("work")}),
        );
        assert_eq!(loaded["error"]["code"], json!(-32012), "{loaded}");
        assert!(loaded["error"]["message"]
            .as_str()
            .unwrap()
            .contains("identity"));
        let duplicate = client.prompt(&session, prompt, Some("original"));
        assert!(
            client.response(duplicate).get("error").is_some(),
            "original input must not be ACKed/rerun"
        );
        let next = client.prompt(&session, "hello", None);
        assert_eq!(client.response(next)["error"]["code"], json!(-32012));
        assert_eq!(f.calls().len(), 1, "no later native effect or rerun");
        eprintln!(
            "control={prompt} response={response} record={record} reopen={loaded} calls={}",
            json!(f.calls())
        );
    }
}

/// Intent: a real failed gate exec or spawn is recognizable as spawn_error;
/// SDK complete never-run evidence permits a later explicitly requested turn,
/// without inventing native identity or starting the refused input again.
#[test]
fn observed_start_failures_settle_without_identity_then_allow_explicit_create() {
    for failure in ["exec", "spawn"] {
        let f = Fixture::new();
        let prepared = f.prepare();
        let cwd = f.path().join("work");
        let mut client = Client::serve(&prepared);
        let session = client.open(&cwd);
        if failure == "exec" {
            std::fs::remove_file(f.path().join("claude")).unwrap();
        } else {
            std::fs::remove_dir(&cwd).unwrap();
        }
        let request = client.prompt(&session, "hello", Some("failed-start"));
        let response = client.response(request);
        assert_eq!(
            response["error"]["code"],
            json!(-32010),
            "{failure}: {response}"
        );
        let native = &response["error"]["data"]["nativeTurn"];
        assert_eq!(native["custody"], json!("complete"), "{response}");
        assert_eq!(native["status"]["kind"], json!("spawn_error"), "{response}");
        assert!(native["status"]["reason"]
            .as_str()
            .unwrap()
            .contains("Failed to spawn Claude provider command"));
        assert_eq!(native["launch_output"]["data_event_count"], json!(0));
        let journal = turn_events(&prepared, &session, native);
        assert_no_reported_identity(&journal);
        eprintln!("control={failure} journal={}", json!(journal));
        let record = session_record(&prepared, &session);
        assert!(record["native_session_id"].is_null(), "{record}");
        assert!(record["native_session_uncertain"].is_null(), "{record}");
        assert_eq!(f.calls().len(), 0);
        drop(client);
        if failure == "exec" {
            std::fs::write(f.path().join("claude"), FAKE_CLAUDE).unwrap();
            std::fs::set_permissions(
                f.path().join("claude"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        } else {
            std::fs::create_dir(&cwd).unwrap();
        }
        let mut client = Client::serve(&prepared);
        client.call(
            "initialize",
            json!({"protocolVersion":2,"info":{"name":"t","version":"0"}}),
        );
        let loaded = client.call("session/resume", json!({"sessionId":session,"cwd":cwd}));
        assert!(loaded.get("result").is_some(), "{loaded}");
        let duplicate = client.prompt(&session, "hello", Some("failed-start"));
        assert_eq!(client.response(duplicate)["error"]["code"], json!(-32010));
        assert_eq!(
            f.calls().len(),
            0,
            "settled failure must not rerun after fake is restored"
        );
        let next = client.prompt(&session, "hello", None);
        let next = message_id(&client.response(next));
        assert_eq!(client.idle_for(&next)["stopReason"], json!("end_turn"));
        let calls = f.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(flag_value(&argv(&calls[0]), "--resume"), None);
        assert_eq!(
            flag_value(&argv(&calls[0]), "--session-id"),
            record["create_native_session_id"]
                .as_str()
                .map(String::from)
        );
        eprintln!(
            "control={failure} response={response} record={record} reopen={loaded} calls={}",
            json!(calls)
        );
    }
}

#[test]
fn observed_identity_survives_no_consumption_and_result_remains_required() {
    let f = Fixture::new();
    let prepared = f.prepare();
    let mut client = Client::serve(&prepared);
    let session = client.open(&f.path().join("work"));
    let request = client.prompt(&session, "noconsume", None);
    assert_eq!(client.response(request)["error"]["code"], json!(-32010));
    let record = session_record(&prepared, &session);
    let native = record["native_session_id"].as_str().unwrap();
    assert!(record["native_session_uncertain"].is_null(), "{record}");
    let request = client.prompt(&session, "noresult", None);
    let id = message_id(&client.response(request));
    let idle = client.idle_for(&id);
    assert_eq!(idle["stopReason"], json!("_oulipoly_native_failed"));
    assert_eq!(
        idle["_meta"]["oulipoly.ai/nativeTurn"]["status"],
        json!({"kind":"exited","code":1})
    );
    assert_eq!(
        flag_value(&argv(&f.calls()[1]), "--resume").as_deref(),
        Some(native)
    );
    eprintln!(
        "control=observed-noresult record={record} idle={idle} calls={}",
        json!(f.calls())
    );
}
