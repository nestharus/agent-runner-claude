// declared_role: orchestration

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
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if sender
                        .send(DrainEvent {
                            channel,
                            bytes: buffer[..count].to_vec(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}
