// declared_role: orchestration, accessor, mapper, predicate

use std::io::Read;
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

#[derive(Debug)]
pub struct DrainEvent {
    pub channel: &'static str,
    pub bytes: Vec<u8>,
}

pub fn spawn_drain<R>(
    channel: &'static str,
    mut reader: R,
    sender: Sender<DrainEvent>,
) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let Some(count) = read_chunk(&mut reader, &mut buffer) else {
                break;
            };
            if drain_send_failed(sender.send(drain_event(channel, &buffer, count))) {
                break;
            }
        }
    })
}

fn read_chunk<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Option<usize> {
    match reader.read(buffer) {
        Ok(0) | Err(_) => None,
        Ok(count) => Some(count),
    }
}

fn drain_event(channel: &'static str, buffer: &[u8], count: usize) -> DrainEvent {
    DrainEvent {
        channel,
        bytes: drain_event_bytes(buffer, count),
    }
}

fn drain_event_bytes(buffer: &[u8], count: usize) -> Vec<u8> {
    buffer[..count].to_vec()
}

fn drain_send_failed(result: Result<(), std::sync::mpsc::SendError<DrainEvent>>) -> bool {
    result.is_err()
}
