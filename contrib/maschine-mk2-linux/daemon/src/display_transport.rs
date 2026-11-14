//! Acknowledged, bounded LCD transactions. LED messages use the immediate path.
//! Each batch is checked before it joins a frame. A frame is applied atomically
//! only after its final batch, so lost packets never become a partially drawn UI.

const PREFIX: [u8; 3] = [0xf0, 0x7d, 0x44];
pub const BATCH_COMMANDS: usize = 16;
pub const FRAME_BYTES: usize = 256 * 1024;
const FRAME_COMMANDS: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    session: u16,
    frame: u16,
    batch: u16,
    count: usize,
    final_batch: bool,
    checksum: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    Consumed,
    Send(Vec<u8>),
    Commit { commands: Vec<Vec<u8>>, ack: Vec<u8> },
}

struct Batch {
    header: Header,
    commands: Vec<Vec<u8>>,
    checksum: u16,
    valid: bool,
    duplicate: bool,
}

#[derive(Default)]
pub struct Transport {
    session: Option<u16>,
    frame: Option<u16>,
    next_batch: u16,
    commands: Vec<Vec<u8>>,
    bytes: usize,
    batch: Option<Batch>,
    last_ack: Option<Header>,
}

fn pair(bytes: &[u8], at: usize) -> u16 {
    u16::from(bytes[at]) | (u16::from(bytes[at + 1]) << 7)
}
fn append_pair(bytes: &mut Vec<u8>, n: u16) {
    bytes.extend_from_slice(&[(n & 127) as u8, (n >> 7) as u8]);
}
pub fn checksum(mut value: u16, bytes: &[u8]) -> u16 {
    for &byte in bytes {
        value = ((u32::from(value) * 31 + u32::from(byte)) & 16383) as u16;
    }
    value
}
fn ack(h: Header) -> Vec<u8> {
    let mut bytes = vec![0xf0, 0x7d, 0x44, 0x41];
    for value in [h.session, h.frame, h.batch, h.checksum] {
        append_pair(&mut bytes, value);
    }
    bytes.push(0xf7);
    bytes
}

impl Transport {
    /// True after the sender's handshake. Unframed graphics then stay ignored.
    pub fn reliable(&self) -> bool {
        self.session.is_some()
    }

    pub fn control(&mut self, bytes: &[u8]) -> Option<Reply> {
        if bytes.len() < 5 || bytes[..3] != PREFIX || ![0x48, 0x54, 0x45].contains(&bytes[3]) {
            return None;
        }
        if *bytes.last()? != 0xf7 || bytes[1..bytes.len() - 1].iter().any(|&b| b > 127) {
            return Some(Reply::Consumed);
        }
        match bytes[3] {
            0x48 if bytes.len() == 7 => {
                let session = pair(bytes, 4);
                *self = Self { session: Some(session), next_batch: 1, ..Default::default() };
                Some(Reply::Send(vec![0xf0, 0x7d, 0x44, 0x52, bytes[4], bytes[5], 0xf7]))
            }
            0x54 if bytes.len() == 15 => {
                self.batch = None;
                let h = Header {
                    session: pair(bytes, 4),
                    frame: pair(bytes, 6),
                    batch: pair(bytes, 8),
                    count: usize::from(bytes[10]),
                    final_batch: bytes[11] == 1,
                    checksum: pair(bytes, 12),
                };
                if self.session != Some(h.session) || !(1..=BATCH_COMMANDS).contains(&h.count) || bytes[11] > 1 || h.batch == 0 {
                    return Some(Reply::Consumed);
                }
                let duplicate = self.last_ack == Some(h);
                if !duplicate {
                    if self.frame != Some(h.frame) {
                        let forward = self.frame.is_none_or(|last| {
                            let delta = (h.frame.wrapping_sub(last)) & 16383;
                            delta > 0 && delta < 8192
                        });
                        if h.batch != 1 || !forward {
                            return Some(Reply::Consumed);
                        }
                        self.frame = Some(h.frame);
                        self.next_batch = 1;
                        self.commands.clear();
                        self.bytes = 0;
                    }
                    if h.batch != self.next_batch {
                        return Some(Reply::Consumed);
                    }
                }
                self.batch = Some(Batch { header: h, commands: Vec::with_capacity(h.count), checksum: 0, valid: true, duplicate });
                Some(Reply::Consumed)
            }
            0x45 if bytes.len() == 13 => {
                let Some(batch) = self.batch.take() else {
                    return Some(Reply::Consumed);
                };
                let h = batch.header;
                if [pair(bytes, 4), pair(bytes, 6), pair(bytes, 8), pair(bytes, 10)] != [h.session, h.frame, h.batch, h.checksum] {
                    return Some(Reply::Consumed);
                }
                if batch.duplicate {
                    return Some(Reply::Send(ack(h)));
                }
                if !batch.valid || batch.commands.len() != h.count || batch.checksum != h.checksum {
                    return Some(Reply::Consumed);
                }
                let added: usize = batch.commands.iter().map(Vec::len).sum();
                if self.bytes + added > FRAME_BYTES || self.commands.len() + batch.commands.len() > FRAME_COMMANDS {
                    return Some(Reply::Consumed);
                }
                self.bytes += added;
                self.commands.extend(batch.commands);
                self.last_ack = Some(h);
                self.next_batch = h.batch + 1;
                if h.final_batch {
                    self.bytes = 0;
                    Some(Reply::Commit { commands: std::mem::take(&mut self.commands), ack: ack(h) })
                } else {
                    Some(Reply::Send(ack(h)))
                }
            }
            _ => Some(Reply::Consumed),
        }
    }

    /// Return whether this packet belongs to the acknowledged graphics path.
    pub fn graphic(&mut self, bytes: &[u8], valid: bool) -> bool {
        if !self.reliable() {
            return false;
        }
        if let Some(batch) = &mut self.batch {
            batch.valid &= valid && bytes.len() <= 64 && batch.commands.len() < batch.header.count;
            if batch.valid {
                batch.checksum = checksum(batch.checksum, bytes);
                batch.commands.push(bytes.to_vec());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hello(t: &mut Transport) {
        assert!(matches!(t.control(&[0xf0, 0x7d, 0x44, 0x48, 1, 0, 0xf7]), Some(Reply::Send(_))))
    }
    fn packets(frame: u16, batch: u16, final_batch: bool, commands: &[Vec<u8>]) -> (Vec<u8>, Vec<u8>) {
        let crc = commands.iter().fold(0, |crc, p| checksum(crc, p));
        let mut begin = vec![0xf0, 0x7d, 0x44, 0x54, 1, 0];
        append_pair(&mut begin, frame);
        append_pair(&mut begin, batch);
        begin.extend_from_slice(&[commands.len() as u8, u8::from(final_batch)]);
        append_pair(&mut begin, crc);
        begin.push(0xf7);
        let mut end = vec![0xf0, 0x7d, 0x44, 0x45, 1, 0];
        append_pair(&mut end, frame);
        append_pair(&mut end, batch);
        append_pair(&mut end, crc);
        end.push(0xf7);
        (begin, end)
    }
    fn commands() -> Vec<Vec<u8>> {
        vec![
            vec![0xf0, 0x7d, 0x44, 0x47, 0, 0, 0xf7],
            vec![0xf0, 0x7d, 0x44, 0x47, 1, 0, 4, 0, 8, 0, 32, 0, 8, 0, 4, 0xf7],
            vec![0xf0, 0x7d, 0x44, 0x47, 0, 1, 0xf7],
        ]
    }
    fn send(t: &mut Transport, frame: u16, batch: u16, last: bool, c: &[Vec<u8>]) -> Option<Reply> {
        let (a, b) = packets(frame, batch, last, c);
        t.control(&a);
        for p in c {
            assert!(t.graphic(p, true))
        }
        t.control(&b)
    }
    #[test]
    fn no_graphics_apply_until_the_complete_frame_commits() {
        let mut t = Transport::default();
        hello(&mut t);
        let c = commands();
        assert!(matches!(send(&mut t, 1, 1, false, &c), Some(Reply::Send(_))));
        assert_eq!(t.commands, c);
        assert!(matches!(send(&mut t,1,2,true,&c),Some(Reply::Commit {commands,..}) if commands.len()==2*c.len()));
        assert!(t.commands.is_empty());
    }
    #[test]
    fn missing_first_middle_and_final_commands_never_commit() {
        let c = commands();
        for missing in 0..c.len() {
            let mut t = Transport::default();
            hello(&mut t);
            let (a, b) = packets(1, 1, true, &c);
            t.control(&a);
            for (i, p) in c.iter().enumerate() {
                if i != missing {
                    t.graphic(p, true);
                }
            }
            assert_eq!(t.control(&b), Some(Reply::Consumed));
            assert!(matches!(send(&mut t, 1, 1, true, &c), Some(Reply::Commit { .. })));
        }
    }
    #[test]
    fn lost_begin_or_end_and_lost_ack_recover_without_reapplying_inversions() {
        let c = commands();
        let (a, b) = packets(1, 1, true, &c);
        let mut t = Transport::default();
        hello(&mut t);
        for p in &c {
            assert!(t.graphic(p, true))
        }
        assert_eq!(t.control(&b), Some(Reply::Consumed));
        t.control(&a);
        for p in &c {
            t.graphic(p, true);
        }
        assert!(matches!(send(&mut t, 1, 1, true, &c), Some(Reply::Commit { .. })));
        // The peer lost the ACK and sends the entire same batch again.
        assert!(matches!(send(&mut t, 1, 1, true, &c), Some(Reply::Send(_))));
        assert!(t.commands.is_empty());
    }
    #[test]
    fn checksum_order_and_invalid_commands_are_checked() {
        let c = commands();
        let (a, b) = packets(1, 1, true, &c);
        let mut t = Transport::default();
        hello(&mut t);
        t.control(&a);
        for p in c.iter().rev() {
            t.graphic(p, true);
        }
        assert_eq!(t.control(&b), Some(Reply::Consumed));
        t.control(&a);
        for p in &c {
            t.graphic(p, false);
        }
        assert_eq!(t.control(&b), Some(Reply::Consumed));
    }
    #[test]
    fn reconnect_and_newer_frames_abandon_uncommitted_old_frames() {
        let c = commands();
        let mut t = Transport::default();
        hello(&mut t);
        send(&mut t, 1, 1, false, &c);
        assert!(matches!(send(&mut t,2,1,true,&c),Some(Reply::Commit{commands,..}) if commands==c));
        assert_eq!(send(&mut t, 1, 1, true, &c), Some(Reply::Consumed));
        hello(&mut t);
        assert!(matches!(send(&mut t, 1, 1, true, &c), Some(Reply::Commit { .. })));
    }
    #[test]
    fn batch_and_frame_memory_are_bounded() {
        let c = commands();
        let mut t = Transport::default();
        hello(&mut t);
        let (a, b) = packets(1, 1, true, &c);
        t.control(&a);
        for _ in 0..10000 {
            t.graphic(&c[0], true);
        }
        assert_eq!(t.batch.as_ref().unwrap().commands.len(), c.len());
        assert_eq!(t.control(&b), Some(Reply::Consumed));
        let c = vec![vec![0; 64]; 16];
        for batch in 1..=256 {
            assert!(matches!(send(&mut t, 2, batch, false, &c), Some(Reply::Send(_))))
        }
        assert_eq!(t.bytes, FRAME_BYTES);
        assert_eq!(send(&mut t, 2, 257, true, &c), Some(Reply::Consumed));
        assert!(t.bytes <= FRAME_BYTES);
    }
    #[test]
    fn hello_must_be_complete_and_its_ack_precedes_any_graphics() {
        let mut t = Transport::default();
        assert!(!t.reliable());
        assert_eq!(t.control(&[0xf0, 0x7d, 0x44, 0x48, 1, 0xf7]), Some(Reply::Consumed));
        assert!(!t.reliable());
        hello(&mut t);
        assert!(t.reliable());
    }
    #[test]
    fn reloading_a_sender_with_the_same_session_starts_from_frame_one() {
        let c = commands();
        let mut t = Transport::default();
        hello(&mut t);
        assert!(matches!(send(&mut t, 120, 1, true, &c), Some(Reply::Commit { .. })));
        hello(&mut t);
        assert!(matches!(send(&mut t,1,1,true,&c),Some(Reply::Commit{commands,..}) if commands==c));
        assert_eq!(t.next_batch, 2);
    }
    #[test]
    fn frame_numbers_wrap_and_old_delayed_frames_stay_ignored() {
        let c = commands();
        let mut t = Transport::default();
        hello(&mut t);
        for frame in [16382, 16383, 0, 1] {
            assert!(matches!(send(&mut t, frame, 1, true, &c), Some(Reply::Commit { .. })));
        }
        for frame in [16383, 0, 16382] {
            assert_eq!(send(&mut t, frame, 1, true, &c), Some(Reply::Consumed));
        }
    }
    #[test]
    fn lost_final_batch_or_ack_never_applies_an_inversion_twice() {
        let c = commands();
        let mut t = Transport::default();
        hello(&mut t);
        assert!(matches!(send(&mut t, 1, 1, false, &c), Some(Reply::Send(_))));
        let (begin, end) = packets(1, 2, true, &c);
        t.control(&begin);
        for p in &c {
            t.graphic(p, true);
        }
        // The final marker disappeared. No commands have been committed yet.
        assert_eq!(t.commands, c);
        let Some(Reply::Commit { commands, ack }) = send(&mut t, 1, 2, true, &c) else { panic!("retry must commit") };
        assert_eq!(commands, [c.clone(), c].concat());
        assert_eq!(t.control(&end), Some(Reply::Consumed));
        // A complete duplicate after losing the ACK only repeats the ACK.
        t.control(&begin);
        assert_eq!(t.control(&end), Some(Reply::Send(ack)));
        assert!(t.commands.is_empty());
    }
}
