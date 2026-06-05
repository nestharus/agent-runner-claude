// declared_role: accessor, formatter, orchestration

use std::io::{self, Write};

use serde_json::{json, Value};

use crate::envelope::CONTRACT;

pub struct EventWriter<W> {
    writer: W,
    request_id: String,
    seq: u64,
}

impl<W: Write> EventWriter<W> {
    pub fn new(writer: W, request_id: &str) -> Self {
        Self {
            writer,
            request_id: request_id.to_string(),
            seq: 1,
        }
    }

    pub fn marker(&mut self, name: &str, value: Value) -> io::Result<()> {
        let seq = self.next_seq();
        let line = marker_line(&self.request_id, seq, name, value)?;
        self.emit_line(&line)
    }

    pub fn heartbeat(&mut self, detail: &str) -> io::Result<()> {
        let seq = self.next_seq();
        let line = heartbeat_line(&self.request_id, seq, detail)?;
        self.emit_line(&line)
    }

    pub fn data(&mut self, kind: &str, bytes: &[u8]) -> io::Result<()> {
        let seq = self.next_seq();
        let line = data_line(&self.request_id, seq, kind, bytes)?;
        self.emit_line(&line)
    }

    pub fn exit(&mut self, status: Value, terminal_signal: Value) -> io::Result<()> {
        let seq = self.next_seq();
        let line = exit_line(&self.request_id, seq, status, terminal_signal)?;
        self.emit_line(&line)
    }

    fn next_seq(&mut self) -> u64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }

    fn emit_line(&mut self, line: &[u8]) -> io::Result<()> {
        self.writer.write_all(line)?;
        self.writer.flush()
    }
}

fn marker_line(request_id: &str, seq: u64, name: &str, value: Value) -> io::Result<Vec<u8>> {
    event_line(json!({
        "contract": CONTRACT,
        "request_id": request_id,
        "seq": seq,
        "time_unix_ms": crate::encoding::now_unix_ms(),
        "kind": "marker",
        "name": name,
        "value": value,
    }))
}

fn heartbeat_line(request_id: &str, seq: u64, detail: &str) -> io::Result<Vec<u8>> {
    event_line(json!({
        "contract": CONTRACT,
        "request_id": request_id,
        "seq": seq,
        "time_unix_ms": crate::encoding::now_unix_ms(),
        "kind": "heartbeat",
        "detail": detail,
    }))
}

fn data_line(request_id: &str, seq: u64, kind: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
    event_line(json!({
        "contract": CONTRACT,
        "request_id": request_id,
        "seq": seq,
        "time_unix_ms": crate::encoding::now_unix_ms(),
        "kind": kind,
        "data_base64": crate::encoding::encode_base64(bytes),
    }))
}

fn exit_line(
    request_id: &str,
    seq: u64,
    status: Value,
    terminal_signal: Value,
) -> io::Result<Vec<u8>> {
    event_line(json!({
        "contract": CONTRACT,
        "request_id": request_id,
        "seq": seq,
        "time_unix_ms": crate::encoding::now_unix_ms(),
        "kind": "exit",
        "status": status,
        "terminal_signal": terminal_signal,
    }))
}

fn event_line(event: Value) -> io::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(&event).map_err(io::Error::other)?;
    line.push(b'\n');
    Ok(line)
}
