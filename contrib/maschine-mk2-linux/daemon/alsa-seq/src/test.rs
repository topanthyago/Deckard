// Copyright (c) 2015 William Light <wrl@illest.net>
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

#[allow(unused_imports)]
use super::*;

#[test]
fn test_seq_open() {
    let handle = SequencerHandle::open("alsa-seq.rs", HandleOpenStreams::Output).unwrap();
}

#[test]
fn test_seq_create_port() {
    let handle = SequencerHandle::open("alsa-seq.rs", HandleOpenStreams::Output).unwrap();

    let port = handle
        .create_port(
            "test output",
            PortCapabilities::PORT_CAPABILITY_READ | PortCapabilities::PORT_CAPABILITY_SUBS_READ,
            PortType::MidiGeneric,
        )
        .unwrap();
    let port2 = handle
        .create_port(
            "test output",
            PortCapabilities::PORT_CAPABILITY_READ | PortCapabilities::PORT_CAPABILITY_SUBS_READ,
            PortType::MidiGeneric,
        )
        .unwrap();
}

#[test]
#[ignore = "requires the host ALSA sequencer; creates isolated test clients"]
fn sysex_direct_roundtrip_and_input_pool() {
    use std::time::{Duration, Instant};
    let sender = SequencerHandle::open("deckard-sysex-send-test", HandleOpenStreams::Duplex).unwrap();
    let receiver = SequencerHandle::open("deckard-sysex-receive-test", HandleOpenStreams::Duplex).unwrap();
    receiver.set_input_pool(1024).unwrap();
    receiver.set_nonblock();
    let capabilities = PortCapabilities::PORT_CAPABILITY_READ
        | PortCapabilities::PORT_CAPABILITY_SUBS_READ
        | PortCapabilities::PORT_CAPABILITY_WRITE
        | PortCapabilities::PORT_CAPABILITY_SUBS_WRITE;
    let output = sender.create_port("isolated output", capabilities, PortType::Application).unwrap();
    let input = receiver.create_port("isolated input", capabilities, PortType::Application).unwrap();
    unsafe {
        assert_eq!(snd_seq_connect_to(sender.raw_handle, output.raw_handle, snd_seq_client_id(receiver.raw_handle), input.raw_handle), 0);
    }
    for size in [7, 13, 63, 64] {
        let mut bytes = vec![1; size];
        bytes[0] = 0xf0;
        bytes[size - 1] = 0xf7;
        output.send_sysex(&bytes).unwrap();
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if let Some(SeqInputEvent::SysEx(got)) = receiver.try_receive_event() {
                assert_eq!(got, bytes, "direct output's packed SysEx pointer");
                break;
            }
            assert!(Instant::now() < deadline, "isolated SysEx receive timeout");
            std::thread::yield_now();
        }
    }
    let id = unsafe { snd_seq_client_id(receiver.raw_handle) };
    let clients = std::fs::read_to_string("/proc/asound/seq/clients").unwrap();
    let start = clients.find(&format!("Client {:3} :", id)).unwrap();
    let client = clients[start..].split("\nClient ").next().unwrap();
    assert!(client.contains("Pool size          : 1024"));
    assert!(client.contains("Alloc failures     : 0"));
}
