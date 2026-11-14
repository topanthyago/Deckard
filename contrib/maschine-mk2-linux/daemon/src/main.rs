//  maschine.rs: user-space drivers for native instruments USB HIDs
//  Copyright (C) 2015 William Light <wrl@illest.net>
//
//  This program is free software: you can redistribute it and/or modify
//  it under the terms of the GNU Lesser General Public License as
//  published by the Free Software Foundation, either version 3 of the
//  License, or (at your option) any later version.
//
//  This program is distributed in the hope that it will be useful,
//  but WITHOUT ANY WARRANTY; without even the implied warranty of
//  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//  GNU Lesser General Public License for more details.
//
//  You should have received a copy of the GNU Lesser General Public
//  License along with this program.  If not, see
//  <http://www.gnu.org/licenses/>.

use std::env;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

use std::time::{Duration, Instant};

extern crate nix;
use nix::fcntl::OFlag;
use nix::poll::{poll, PollFd, PollFlags};
use nix::{fcntl, sys};

extern crate alsa_seq;
extern crate midi;
use alsa_seq::SeqInputEvent;
use alsa_seq::*;
use midi::*;

extern crate hsl;
use hsl::HSL;

#[macro_use(osc_args)]
extern crate tinyosc;
use tinyosc as osc;

mod base;
mod cc_math;
mod config;
mod devices;
mod display;
mod display_transport;
mod font;
mod hid_feature;
mod hid_stats;
mod midi_parse;
mod watchdog;
use config::MaschineConfig;

use crate::base::{Maschine, MaschineButton, MaschineHandler};

// A private, non-commercial SysEx frame carries Deckard's LED state.
// Generic MIDI inputs ignore SysEx, avoiding notes and CC feedback loops.
const DECKARD_BUTTONS: [MaschineButton; 47] = [
    MaschineButton::GroupA,
    MaschineButton::GroupB,
    MaschineButton::GroupC,
    MaschineButton::GroupD,
    MaschineButton::GroupE,
    MaschineButton::GroupF,
    MaschineButton::GroupG,
    MaschineButton::GroupH,
    MaschineButton::Play,
    MaschineButton::Rec,
    MaschineButton::Shift,
    MaschineButton::Pageleft,
    MaschineButton::Pageright,
    MaschineButton::Restart,
    MaschineButton::Control,
    MaschineButton::Step,
    MaschineButton::Browse,
    MaschineButton::Sampling,
    MaschineButton::All,
    MaschineButton::Auto,
    MaschineButton::F1,
    MaschineButton::F2,
    MaschineButton::F3,
    MaschineButton::F4,
    MaschineButton::F5,
    MaschineButton::F6,
    MaschineButton::F7,
    MaschineButton::F8,
    MaschineButton::Scene,
    MaschineButton::Pattern,
    MaschineButton::Padmode,
    MaschineButton::Navigate,
    MaschineButton::Duplicate,
    MaschineButton::Select,
    MaschineButton::Solo,
    MaschineButton::Mute,
    MaschineButton::Volume,
    MaschineButton::Swing,
    MaschineButton::Tempo,
    MaschineButton::Navleft,
    MaschineButton::Navright,
    MaschineButton::Enter,
    MaschineButton::Noterepeat,
    MaschineButton::Grid,
    MaschineButton::Stepright,
    MaschineButton::Stepleft,
    MaschineButton::Erase,
];

fn deckard_button_led(bytes: &[u8]) -> Option<(MaschineButton, u32, f32)> {
    let [0xf0, 0x7d, 0x44, 0x4b, id, r, g, b, 0xf7] = *bytes else { return None };
    if r > 127 || g > 127 || b > 127 {
        return None;
    }
    let button = *DECKARD_BUTTONS.get(id as usize)?;
    let scale = |v: u8| u32::from(v) * 255 / 127;
    let color = (scale(r) << 16) | (scale(g) << 8) | scale(b);
    let brightness = if id < 8 { 1.0 } else { f32::from(r) / 127.0 };
    Some((button, color, brightness))
}

fn deckard_pad_led(bytes: &[u8]) -> Option<(usize, u32)> {
    let [0xf0, 0x7d, 0x44, 0x4b, id, r, g, b, 0xf7] = *bytes else { return None };
    if !(48..64).contains(&id) || r > 127 || g > 127 || b > 127 {
        return None;
    }
    let scale = |v: u8| u32::from(v) * 255 / 127;
    Some(((id - 48) as usize, (scale(r) << 16) | (scale(g) << 8) | scale(b)))
}

// A second RGB emitter sits beneath each A-H button. These complementary
// triplets were identified individually on the connected MK2, 2026-11-13.
fn deckard_group_indicator(bytes: &[u8]) -> Option<(usize, [u8; 3])> {
    let [0xf0, 0x7d, 0x44, 0x4b, id, r, g, b, 0xf7] = *bytes else { return None };
    if !(65..=72).contains(&id) || r > 127 || g > 127 || b > 127 {
        return None;
    }
    const STARTS: [usize; 8] = [4, 10, 16, 19, 28, 31, 40, 43];
    let scale = |value: u8| (u16::from(value) * 255 / 127) as u8;
    Some((STARTS[(id - 65) as usize], [scale(r), scale(g), scale(b)]))
}

fn deckard_screen_line(bytes: &[u8]) -> Option<(usize, usize, &str)> {
    if bytes.len() < 7
        || bytes.len() > 47
        || bytes[..4] != [0xf0, 0x7d, 0x44, 0x53]
        || *bytes.last()? != 0xf7
        || bytes[4] > 1
        || bytes[5] > 7
    {
        return None;
    }
    let text = &bytes[6..bytes.len() - 1];
    if !text.iter().all(|b| (32..=126).contains(b)) {
        return None;
    }
    Some((bytes[4] as usize, bytes[5] as usize, std::str::from_utf8(text).ok()?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenGraphic<'a> {
    Clear { screen: usize },
    Text { screen: usize, x: usize, y: usize, scale: usize, text: &'a str },
    WaveColumns { screen: usize, x: usize, y: usize, h: usize, columns: &'a [u8] },
    Rect { screen: usize, x: usize, y: usize, w: usize, h: usize, style: usize },
}

fn unpack14(lo: u8, hi: u8) -> usize {
    usize::from(lo & 0x7f) | (usize::from(hi & 0x7f) << 7)
}

/// Binary drawing frames for the MK2 monochrome panels. The framebuffer remains
/// in the bridge, so this carries primitives rather than pixels and keeps the
/// MIDI path small enough to coexist with pad feedback.
fn deckard_screen_graphic(bytes: &[u8]) -> Option<ScreenGraphic<'_>> {
    if bytes.len() < 7
        || bytes[..4] != [0xf0, 0x7d, 0x44, 0x47]
        || *bytes.last()? != 0xf7
        || bytes[1..bytes.len() - 1].iter().any(|&b| b > 127)
    {
        return None;
    }
    match bytes[4] {
        0 if bytes.len() == 7 && bytes[5] <= 1 => Some(ScreenGraphic::Clear { screen: bytes[5] as usize }),
        1 if bytes.len() == 16 && bytes[5] <= 1 => {
            let (x, y, w, h) = (
                unpack14(bytes[6], bytes[7]),
                unpack14(bytes[8], bytes[9]),
                unpack14(bytes[10], bytes[11]),
                unpack14(bytes[12], bytes[13]),
            );
            let style = usize::from(bytes[14]);
            if x < display::WIDTH && y < display::HEIGHT && w > 0 && h > 0 && style <= 5 {
                Some(ScreenGraphic::Rect { screen: bytes[5] as usize, x, y, w, h, style })
            } else {
                None
            }
        }
        2 if (12..=54).contains(&bytes.len()) && bytes[5] <= 1 => {
            let (x, y) = (unpack14(bytes[6], bytes[7]), unpack14(bytes[8], bytes[9]));
            let scale = usize::from(bytes[10]);
            let text = &bytes[11..bytes.len() - 1];
            if x < display::WIDTH && y < display::HEIGHT && (1..=2).contains(&scale) && text.iter().all(|b| (32..=126).contains(b)) {
                Some(ScreenGraphic::Text { screen: bytes[5] as usize, x, y, scale, text: std::str::from_utf8(text).ok()? })
            } else {
                None
            }
        }
        3 if (15..=63).contains(&bytes.len()) && (bytes.len() - 11) % 4 == 0 && bytes[5] <= 1 => {
            let x = unpack14(bytes[6], bytes[7]);
            let (y, h) = (usize::from(bytes[8]), usize::from(bytes[9]));
            let columns = &bytes[10..bytes.len() - 1];
            if x + columns.len() / 4 <= display::WIDTH
                && (3..=64).contains(&h)
                && y + h <= display::HEIGHT
                && columns.chunks_exact(4).all(|c| c[0] <= c[1] && usize::from(c[1]) < h && unpack14(c[2], c[3]) <= 511)
            {
                Some(ScreenGraphic::WaveColumns { screen: bytes[5] as usize, x, y, h, columns })
            } else {
                None
            }
        }
        _ => None,
    }
}
// Compact waveform columns carry geometry and overlay flags, keeping a full
// two-panel waveform under fifty bounded MIDI packets. Each column is still
// erased and redrawn in the existing framebuffer, using its dirty rectangles.
fn apply_screen_graphic(dev: &mut dyn Maschine, bytes: &[u8]) {
    match deckard_screen_graphic(bytes) {
        Some(ScreenGraphic::Clear { screen }) => dev.display_fb_clear(screen),
        Some(ScreenGraphic::Text { screen, x, y, scale, text }) => dev.display_fb_text(screen, x, y, scale, false, text),
        Some(ScreenGraphic::Rect { screen, x, y, w, h, style }) => dev.display_fb_rect(screen, x, y, w, h, style),
        Some(ScreenGraphic::WaveColumns { screen, x, y, h, columns }) => deckard_wave_columns(dev, screen, x, y, h, columns),
        None => {}
    }
}

fn deckard_wave_columns(dev: &mut dyn Maschine, screen: usize, x: usize, y: usize, h: usize, columns: &[u8]) {
    for (offset, column) in columns.chunks_exact(4).enumerate() {
        let px = x + offset;
        let (top, bottom) = (usize::from(column[0]), usize::from(column[1]));
        let flags = unpack14(column[2], column[3]);
        dev.display_fb_rect(screen, px, y, 1, h, 5);
        if flags & 1 != 0 {
            dev.display_fb_rect(screen, px, y + top, 1, bottom - top + 1, 1);
        }
        if flags & 2 != 0 {
            dev.display_fb_rect(screen, px, y, 1, h, 4);
        }
        if flags & 4 != 0 {
            dev.display_fb_rect(screen, px, y + 1, 1, 1, 1);
            dev.display_fb_rect(screen, px, y + h - 2, 1, 1, 1);
            if flags & 8 != 0 {
                dev.display_fb_rect(screen, px, y + 1, 1, h - 2, 1);
            }
        }
        if flags & 16 != 0 {
            for yy in (y..y + h).step_by(2) {
                dev.display_fb_rect(screen, px, yy, 1, 1, 1);
            }
        }
        if flags & 32 != 0 {
            dev.display_fb_rect(screen, px, y, 1, h, 1);
        }
        if flags & 64 != 0 {
            dev.display_fb_rect(screen, px, y, 1, 1, 1);
            dev.display_fb_rect(screen, px, y + h - 1, 1, 1, 1);
            if flags & 128 != 0 {
                dev.display_fb_rect(screen, px, y, 1, h, 1);
            }
        }
        if flags & 256 != 0 {
            dev.display_fb_rect(screen, px, y, 1, h, 4);
        }
    }
}

// The screen protocol uses 6-pixel fixed-width character cells. Return only
// the changed span; shortening a line also clears its former trailing cells.
fn changed_cells(old: &str, new: &str) -> Option<(usize, usize)> {
    let (old, new) = (old.as_bytes(), new.as_bytes());
    let len = old.len().max(new.len());
    let first = (0..len).find(|&i| old.get(i) != new.get(i))?;
    let last = (first..len).rfind(|&i| old.get(i) != new.get(i))?;
    Some((first, last + 1))
}

fn deckard_pad_mode(bytes: &[u8]) -> Option<u8> {
    match bytes {
        [0xf0, 0x7d, 0x44, 0x4b, 64, mode @ 0..=3, 0, 0, 0xf7] => Some(*mode),
        _ => None,
    }
}

fn pad_modifier(button: MaschineButton) -> u8 {
    match button {
        MaschineButton::Mute => 1,
        MaschineButton::Solo => 2,
        MaschineButton::Select => 4,
        MaschineButton::Scene => 8,
        MaschineButton::Pattern => 16,
        MaschineButton::Duplicate => 32,
        MaschineButton::Erase => 64,
        _ => 0,
    }
}

fn wrap_encoder(current: i32, delta: i32) -> u8 {
    (current + delta).rem_euclid(128) as u8
}

fn pressed_pad_color(color: u32) -> u32 {
    if color == 0 {
        return 0x404040;
    }
    let channel = |shift: u32| (color >> shift) & 0xff_u32;
    let maximum = channel(16).max(channel(8)).max(channel(0));
    // One gain keeps the hue. Saturating channels independently turned a
    // bright lime into yellow when red and green both reached 255.
    let gain = 2.0_f32.min(255.0 / maximum as f32);
    let boost = |shift: u32| (channel(shift) as f32 * gain).round() as u32;
    (boost(16) << 16) | (boost(8) << 8) | boost(0)
}

fn button_shown(id: usize, color: u32, brightness: f32, held: bool) -> (u32, f32) {
    if !held {
        return (color, brightness);
    }
    if id < 8 {
        (pressed_pad_color(color), 1.0)
    } else {
        (color, 1.0)
    }
}

fn ev_loop(dev: &mut dyn Maschine, mhandler: &mut MHandler, dev_path: &str) {
    let mut fds = [
        PollFd::new(dev.get_fd(), PollFlags::POLLIN),
        PollFd::new(mhandler.osc_socket.as_raw_fd(), PollFlags::POLLIN),
        PollFd::new(mhandler.seq_in_fd, PollFlags::POLLIN),
    ];

    // INSTANT, NOT SystemTime, since 2026-09-03. `SystemTime::elapsed()`
    // returns a Result and these four were all `.unwrap()`, so a BACKWARD
    // clock step panicked the daemon - and this Pi has no RTC: fake-hwclock
    // restores a stale time at boot and NTP corrects it later, measured eight
    // days out on 2026-08-30. Every one of these is an interval, which is what
    // a monotonic clock is for; none of them wants a wall-clock date.
    let mut now = Instant::now();
    let mut now_display = Instant::now();
    // LED reports are dirty-tracked separately, so a shorter coalescing
    // interval improves feedback without rewriting all three reports.
    let timer_interval = Duration::from_millis(10);
    let display_interval = Duration::from_millis(40);
    let led_latency_log = env::var_os("MASCHINE_LED_LATENCY").is_some();
    let mut led_pending: Option<Instant> = None;
    let mut last_report = Instant::now();
    let mut reopens: u64 = 0;
    // Storm state for the input watchdog. `consecutive` counts reopens since
    // input last flowed healthily; it is what the backoff is computed from, and
    // it is NOT the same as `reopens`, which stays a lifetime total because
    // `journalctl | grep -c reopened` is this project's primary diagnostic.
    let mut consecutive: u64 = 0;
    let mut last_reopen = Instant::now();
    let mut backoff_until = Instant::now();
    let mut announced_backoff = Duration::from_millis(0);
    // 50 ms unless overridden. The override exists for ONE experiment that
    // cannot be run any other way (item 79, 2026-09-08): with the watchdog's
    // window widened, the input-gap histogram shows how long the device's
    // dropouts ACTUALLY last, and therefore whether the reopen is what ends
    // them or they end by themselves. A bad value is ignored, and there is no
    // config key on purpose - this is an instrument, not a setting.
    let input_timeout = env::var("MASCHINE_INPUT_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&ms| ms >= 50 && ms <= 10_000)
        .map(|ms| {
            println!("watchdog: input_timeout overridden to {}ms", ms);
            Duration::from_millis(ms)
        })
        .unwrap_or_else(|| Duration::from_millis(50));
    // ITEM 79 instrumentation, 2026-09-08: WHICH SIDE OF THE FD the window is
    // measuring. `watchdog::StallWindow` explains the shapes; the flags are
    // accumulated because a POLLERR or POLLHUP anywhere inside the window is
    // the device's own answer and it is gone by the time we look.
    let mut window = watchdog::StallWindow::default();
    let mut lap_mark = Instant::now();
    let mut stall_revents = PollFlags::empty();
    // The gap distribution of the input stream itself, off unless asked for.
    // It measures the "~750 reports/s unconditionally" claim the 50 ms timeout
    // rests on, which nothing has ever done.
    let input_stats = env::var("MASCHINE_INPUT_STATS").is_ok();
    let mut gaps = watchdog::InputGaps::default();
    // ITS OWN CLOCK, AND THAT IS THE WHOLE POINT. `last_report` is the
    // WATCHDOG's bookkeeping and the watchdog resets it after every reopen, so
    // a histogram built on it reported `max gap 6ms` for a minute containing
    // two stalls - the dropout was hidden by the recovery from it. Measured
    // 2026-09-08 18:10, and it is exactly the shape of the whole item.
    let mut last_seen = Instant::now();
    let mut gaps_since = Instant::now();
    let gaps_interval = Duration::from_secs(60);
    // ITEM 79/22, 2026-09-08. The 2026-08-22 write-budget curve is denominated
    // in reopens per minute and shows the rate DOUBLING as our HID writes
    // saturate - but it never ran the other end of the axis, so nobody knows
    // what the rate is with NO writes at all. This holds every HID write back
    // (LEDs go stale, screens stop repainting; input is untouched) so the
    // dropout rate can be measured against zero write traffic. An experiment,
    // and not a mode anything should ship enabled.
    let no_hid_writes = env::var("MASCHINE_NO_HID_WRITES").is_ok();
    if no_hid_writes {
        println!("watchdog: HID WRITES SUPPRESSED - lights and screens are frozen");
    }
    loop {
        let mark = Instant::now();
        window.lap(mark - lap_mark);
        lap_mark = mark;
        // poll() returns EINTR on any signal the process is handed, and
        // `.unwrap()` on that ended the daemon. There is nothing to do about
        // an interrupted wait except take the next lap.
        if let Err(err) = poll(&mut fds, 16) {
            if err != nix::errno::Errno::EINTR {
                println!("poll failed: {} - retrying", err);
            }
            continue;
        }

        stall_revents |= fds[0].revents().unwrap_or_else(PollFlags::empty);

        if fds[0].revents().unwrap_or_else(PollFlags::empty).contains(PollFlags::POLLIN) {
            dev.readable(mhandler);
            if input_stats {
                gaps.record(last_seen.elapsed());
                last_seen = Instant::now();
            }
            last_report = Instant::now();
            window.reset();
            stall_revents = PollFlags::empty();
            // Input is flowing. If it has been flowing long enough, the storm
            // is over and the next isolated stall gets an immediate retry
            // again - which is the common, recovering case and must stay fast.
            if consecutive > 0 && watchdog::storm_is_over(last_reopen.elapsed()) {
                println!("watchdog: input recovered after {} consecutive reopens", consecutive);
                consecutive = 0;
                announced_backoff = Duration::from_millis(0);
            }
        }

        // Input watchdog. The device streams ~750 reports/s unconditionally, so
        // any silence longer than this means the kernel hidraw layer has stopped
        // delivering to our fd (verified with usbmon: the URBs keep completing
        // with data while read() returns EAGAIN forever). A fresh open() is the
        // only known recovery.
        //
        // MEASURED 2026-09-08, both halves of that: 724-745 reports/s over
        // seven consecutive minutes, 95 % of gaps under 2 ms and the worst
        // normal gap 22 ms — so a 50 ms silence is 35x the spacing and the
        // distribution is bimodal with nothing in between. And with the window
        // widened to 2 s the stream never came back on its own, so "the only
        // known recovery" is now the only MEASURED one.
        if last_report.elapsed() >= input_timeout && Instant::now() >= backoff_until {
            let gap = last_report.elapsed();
            // Close BEFORE reopening: usbhid only tears down and resubmits the
            // interrupt URB when the device user count drops to zero, and that
            // teardown is what actually revives the stream. Opening first left
            // one user held throughout, and the reopen had no effect at all.
            // On unplug the reopen may fail repeatedly. Never close the same
            // numeric fd twice: another library could reuse it in between.
            if dev.get_fd() >= 0 {
                let _ = nix::unistd::close(dev.get_fd());
                dev.set_fd(-1);
                fds[0] = PollFd::new(-1, PollFlags::POLLIN);
            }
            match fcntl::open(Path::new(dev_path), OFlag::O_RDWR | OFlag::O_NONBLOCK, sys::stat::Mode::empty()) {
                Ok(new_fd) => {
                    dev.set_fd(new_fd);
                    mhandler.release_pads(dev);
                    dev.invalidate_lights();
                    fds[0] = PollFd::new(new_fd, PollFlags::POLLIN);
                    reopens += 1;
                    // The leading text is load-bearing: `grep -c "input
                    // stalled, reopened"` is this project's primary diagnostic
                    // and replug-heal-test.py counts it. Item 79's numbers are
                    // appended, never substituted.
                    println!(
                        "watchdog: input stalled, reopened {} (reopen #{}) gap={}ms laps={} lap_max={}ms revents={:?}",
                        dev_path,
                        reopens,
                        gap.as_millis(),
                        window.laps,
                        window.longest_lap.as_millis(),
                        stall_revents
                    );
                }
                Err(err) => {
                    if consecutive < 3 || consecutive % 30 == 0 {
                        println!("watchdog: reopen of {} failed: {}", dev_path, err);
                    }
                }
            }
            // Counted on failure too: a reopen that cannot even open the node
            // is the strongest possible reason to stop hammering it.
            consecutive += 1;
            last_reopen = Instant::now();
            let delay = watchdog::reopen_delay(consecutive);
            backoff_until = last_reopen + delay;
            // Announced only when the step CHANGES. Every reopen still prints
            // its own line above, so `grep -c reopened` keeps counting exactly
            // what it always counted; this line explains the spacing between
            // them rather than replacing them.
            if delay != announced_backoff {
                announced_backoff = delay;
                if delay > Duration::from_millis(0) {
                    println!("watchdog: backing off {}ms after {} consecutive reopens", delay.as_millis(), consecutive);
                }
            }
            last_report = Instant::now();
            window.reset();
            stall_revents = PollFlags::empty();
        }

        if fds[1].revents().unwrap_or_else(PollFlags::empty).contains(PollFlags::POLLIN) {
            mhandler.recv_osc_msg(dev);
        }

        if mhandler.seq_in_fd >= 0 && fds[2].revents().unwrap_or_else(PollFlags::empty).contains(PollFlags::POLLIN) {
            let mut midi_led_changed = false;
            while let Some(ev) = mhandler.seq_handle.try_receive_event() {
                match ev {
                    SeqInputEvent::NoteOn { note, velocity, .. } if note < 16 && !mhandler.external_pad_leds => {
                        let brightness = if velocity == 0 { PAD_RELEASED_BRIGHTNESS } else { velocity as f32 / 127.0 };
                        let color = mhandler.pad_color();
                        dev.set_pad_light(note as usize, color, brightness);
                    }
                    SeqInputEvent::NoteOff { note, .. } if note < 16 && !mhandler.external_pad_leds => {
                        let color = mhandler.pad_color();
                        dev.set_pad_light(note as usize, color, PAD_RELEASED_BRIGHTNESS);
                    }
                    SeqInputEvent::SysEx(bytes) => {
                        if let Some(reply) = mhandler.display_transport.control(&bytes) {
                            let ack = match reply {
                                display_transport::Reply::Consumed => None,
                                display_transport::Reply::Send(ack) => Some(ack),
                                display_transport::Reply::Commit { commands, ack } => {
                                    for command in &commands {
                                        apply_screen_graphic(dev, command);
                                    }
                                    Some(ack)
                                }
                            };
                            if let Some(ack) = ack {
                                if let Err(error) = mhandler.seq_port.send_sysex(&ack) {
                                    println!("display ACK send failed: {:?}", error);
                                }
                            }
                        } else if bytes.starts_with(&[0xf0, 0x7d, 0x44, 0x47]) && mhandler.display_transport.reliable() {
                            let valid = deckard_screen_graphic(&bytes).is_some();
                            mhandler.display_transport.graphic(&bytes, valid);
                        } else if let Some((button, color, brightness)) = deckard_button_led(&bytes) {
                            midi_led_changed = true;
                            if led_latency_log && led_pending.is_none() {
                                led_pending = Some(Instant::now());
                            }
                            let id = bytes[4] as usize;
                            mhandler.button_colors[id] = (color, brightness);
                            let (color, brightness) = button_shown(id, color, brightness, mhandler.button_held[id]);
                            dev.set_button_light(button, color, brightness);
                        } else if let Some((pad, color)) = deckard_pad_led(&bytes) {
                            midi_led_changed = true;
                            if led_latency_log && led_pending.is_none() {
                                led_pending = Some(Instant::now());
                            }
                            mhandler.pad_colors[pad] = color;
                            let shown = if mhandler.pad_held[pad] { pressed_pad_color(color) } else { color };
                            dev.set_pad_light(pad, shown, 1.0);
                        } else if let Some((start, rgb)) = deckard_group_indicator(&bytes) {
                            midi_led_changed = true;
                            if led_latency_log && led_pending.is_none() {
                                led_pending = Some(Instant::now());
                            }
                            for (offset, value) in rgb.iter().enumerate() {
                                dev.set_raw_light(3, start + offset, *value);
                            }
                        } else if let Some((screen, row, text)) = deckard_screen_line(&bytes) {
                            if let Some((first, end)) = changed_cells(&mhandler.screen_lines[screen][row], text) {
                                let x = 1 + first * display::char_w(1);
                                dev.display_fb_rect(screen, x, row * 8, (end - first) * display::char_w(1), 8, 5);
                                if first < text.len() {
                                    dev.display_fb_text(screen, x, row * 8, 1, false, &text[first..end.min(text.len())]);
                                }
                                mhandler.screen_lines[screen][row] = text.to_string();
                            }
                        } else if let Some(graphic) = deckard_screen_graphic(&bytes) {
                            match graphic {
                                ScreenGraphic::Clear { screen } => dev.display_fb_clear(screen),
                                ScreenGraphic::WaveColumns { screen, x, y, h, columns } => {
                                    deckard_wave_columns(dev, screen, x, y, h, columns)
                                }
                                ScreenGraphic::Text { screen, x, y, scale, text } => dev.display_fb_text(screen, x, y, scale, false, text),
                                ScreenGraphic::Rect { screen, x, y, w, h, style } => dev.display_fb_rect(screen, x, y, w, h, style),
                            }
                        } else if let Some(mode) = deckard_pad_mode(&bytes) {
                            mhandler.pad_mode = mode;
                        }
                    }
                    SeqInputEvent::Clock => {
                        let _ = mhandler.seq_port.send_message(&Message::TimingClock);
                        mhandler.seq_handle.drain_output();
                    }
                    SeqInputEvent::Start => {
                        let _ = mhandler.seq_port.send_message(&Message::Start);
                        mhandler.seq_handle.drain_output();
                    }
                    SeqInputEvent::Stop => {
                        let _ = mhandler.seq_port.send_message(&Message::Stop);
                        mhandler.seq_handle.drain_output();
                    }
                    _ => {}
                }
            }
            // Coalesce the received MIDI batch, then write its LED reports
            // immediately. The timer below still coalesces local input and OSC.
            if midi_led_changed && !no_hid_writes {
                dev.write_lights();
                if let Some(at) = led_pending.take() {
                    println!("LED MIDI-to-HID: {} us", at.elapsed().as_micros());
                }
                now = Instant::now();
            }
        }

        if input_stats && gaps_since.elapsed() >= gaps_interval {
            println!("{}", gaps.line(gaps_since.elapsed()));
            gaps.reset();
            gaps_since = Instant::now();
        }

        if now.elapsed() >= timer_interval {
            if !no_hid_writes {
                dev.write_lights();
                if let Some(at) = led_pending.take() {
                    println!("LED MIDI-to-HID: {} us", at.elapsed().as_micros());
                }
            }
            now = Instant::now();
        }
        if now_display.elapsed() >= display_interval {
            // Normal rendering stays off - it issued ~180 writes/s. Each
            // report is 9 header bytes plus a 256-byte payload = 265, and a
            // both-screen rebuild is 16 of them (this comment said 521 bytes
            // until 2026-08-30; the write COUNT was always the half that
            // mattered). Calibration redraws are rate-limited to this timer.
            if !no_hid_writes {
                dev.calib_flush();
            }
            // Screen framebuffers written over OSC land here too, for the same
            // reason: pushing them from the OSC handler would interleave HID
            // writes with the input reads.
            if !no_hid_writes {
                dev.display_fb_flush();
            }
            now_display = Instant::now();
        }
    }
}

fn usage(prog_name: &String) {
    println!("usage: {} <hidraw device>", prog_name);
}

/// Where the big encoder's counter and reported value live.
///
/// roller_status and roller_value are both [i32; 9] for eight encoders. The
/// ninth slot was always spare; the big encoder now uses it, so it shares the
/// jump-rejection and accumulation the other eight have had all along.
const BIG_ENC_SLOT: usize = 8;

const PAD_RELEASED_BRIGHTNESS: f32 = 0.015;

/// Minimum gap between two aftertouch sends for the SAME pad.
///
/// The device streams pressure at ~750 reports/s (endpoint ceiling is 8,000/s:
/// high speed, bInterval 1, measured on the rig 2026-08-30). Every send is a
/// MIDI message into ZynMidiRouter, and the driver's `midi_event` holds its
/// lock for the whole event — so the raw rate is a hazard, not a resolution.
/// 25 ms caps one held pad at ~40 msg/s, a 19x reduction, which is still four
/// times the driver's own ~10 Hz consumption of it.
const AFTERTOUCH_MIN_INTERVAL: Duration = Duration::from_millis(25);

#[allow(dead_code)]
enum PressureShape {
    Linear,
    Exponential(f32),
    Constant(f32),
}

struct MHandler<'a> {
    color: HSL,

    seq_handle: &'a SequencerHandle,
    seq_port: &'a SequencerPort<'a>,
    drum_port: &'a SequencerPort<'a>,

    pressure_shape: PressureShape,
    send_aftertouch: bool,

    osc_socket: &'a UdpSocket,
    osc_outgoing_addr: SocketAddr,

    seq_in_fd: RawFd,

    pad_notes: [u8; 16],
    encoder_ccs: [u16; 8],
    external_pad_leds: bool,
    pad_modifiers: u8,
    pad_mode: u8,
    pad_note_down: [bool; 16],
    pad_colors: [u32; 16],
    pad_held: [bool; 16],
    button_colors: [(u32, f32); 47],
    button_held: [bool; 47],
    screen_lines: [[String; 8]; 2],
    display_transport: display_transport::Transport,
    /// Last aftertouch value sent per pad, and when. Both are needed: the
    /// change gate alone still lets a slowly-moving finger send at the report
    /// rate, and the time gate alone re-sends a value nobody changed.
    at_last_val: [u8; 16],
    at_last_sent: [Option<Instant>; 16],
}

/// THE PANEL, IN ONE TABLE: OSC token, enum variant, and the CC the driver
/// sees on press and release.
///
/// It replaces three hand-maintained things that had to agree and that nothing
/// checked: `osc_button_to_btn_map`, its hand-written inverse
/// `btn_to_osc_button_map`, and about forty identical `RPN7` match arms that
/// differed only in a number. **The CC map is now greppable** - which matters
/// because this project's rule is that a CC is measured hardware fact and is
/// never read off a token name, so the one place it is written down should be
/// a place a person can find.
///
/// `None` means the generic sender must NOT fire for that token, because
/// something else already handles it and does more than send one CC:
///
/// * `encoder` has no CC at all - the big encoder's delta path is separate.
/// * `pad_mode` sends CC 27 through its own match arm.
/// * `group_a`..`group_h` send `cc_math::group_cc(n)` through `seq_port` AND
///   move the pad note base, so they are real behaviour, not a table row.
///
/// The order is the order the old forward table had, which is roughly the
/// panel's own reading order. Nothing depends on it - both lookups are linear
/// scans over fifty-two rows, on a path that runs once per button edge.
const PANEL_BUTTONS: &[(&str, MaschineButton, Option<u16>)] = &[
    ("restart", MaschineButton::Restart, Some(7)),
    ("step_left", MaschineButton::Stepleft, Some(5)),
    ("step_right", MaschineButton::Stepright, Some(6)),
    ("grid", MaschineButton::Grid, Some(4)),
    ("play", MaschineButton::Play, Some(1)),
    ("rec", MaschineButton::Rec, Some(3)),
    ("stop", MaschineButton::Erase, Some(2)),
    ("shift", MaschineButton::Shift, Some(49)),
    ("browse", MaschineButton::Browse, Some(8)),
    ("sampling", MaschineButton::Sampling, Some(9)),
    ("note_repeat", MaschineButton::Noterepeat, Some(10)),
    ("encoder", MaschineButton::Encoder, None),
    ("f1", MaschineButton::F1, Some(39)),
    ("f2", MaschineButton::F2, Some(40)),
    ("f3", MaschineButton::F3, Some(41)),
    ("f4", MaschineButton::F4, Some(42)),
    ("f5", MaschineButton::F5, Some(43)),
    ("f6", MaschineButton::F6, Some(44)),
    ("f7", MaschineButton::F7, Some(45)),
    ("f8", MaschineButton::F8, Some(46)),
    ("swing", MaschineButton::Swing, Some(50)),
    ("step", MaschineButton::Step, Some(32)),
    ("volume", MaschineButton::Volume, Some(51)),
    ("enter", MaschineButton::Enter, Some(36)),
    ("auto", MaschineButton::Auto, Some(37)),
    ("all", MaschineButton::All, Some(38)),
    ("navigate", MaschineButton::Navigate, Some(34)),
    ("tempo", MaschineButton::Tempo, Some(35)),
    // "stop" APPEARED TWICE in the old forward table - once live and once
    // commented out - and the commented copy is not restored here. Note what
    // the live row says: the OSC token "stop" is the ERASE button. That is the
    // daemon's token names being attached to the opposite physical buttons,
    // which is measured fact and is why a CC is never read off a name.
    ("control", MaschineButton::Control, Some(11)),
    ("nav", MaschineButton::Nav, Some(12)),
    ("nav_left", MaschineButton::Navleft, Some(13)),
    ("nav_right", MaschineButton::Navright, Some(14)),
    ("main", MaschineButton::Main, Some(24)),
    ("scene", MaschineButton::Scene, Some(25)),
    ("pattern", MaschineButton::Pattern, Some(26)),
    ("pad_mode", MaschineButton::Padmode, None),
    ("view", MaschineButton::View, Some(28)),
    ("duplicate", MaschineButton::Duplicate, Some(29)),
    ("select", MaschineButton::Select, Some(30)),
    ("solo", MaschineButton::Solo, Some(31)),
    ("mute", MaschineButton::Mute, Some(33)),
    ("group_a", MaschineButton::GroupA, None),
    ("group_b", MaschineButton::GroupB, None),
    ("group_c", MaschineButton::GroupC, None),
    ("group_d", MaschineButton::GroupD, None),
    ("group_e", MaschineButton::GroupE, None),
    ("group_f", MaschineButton::GroupF, None),
    ("group_g", MaschineButton::GroupG, None),
    ("group_h", MaschineButton::GroupH, None),
    ("page_right", MaschineButton::Pageright, Some(47)),
    ("page_left", MaschineButton::Pageleft, Some(48)),
];

/// The CC a plain button sends, or `None` if it is not a plain button.
fn panel_button_cc(osc_button: &str) -> Option<u16> {
    PANEL_BUTTONS.iter().find(|(name, _, _)| *name == osc_button).and_then(|(_, _, cc)| *cc)
}

/// The MIDI message one button edge produces, or `None` if this token is
/// handled somewhere that does more than send a CC.
///
/// A PURE FUNCTION SO A TEST CAN CALL IT, and that is the whole reason it is
/// not inlined at the one call site. `MHandler` needs an ALSA port and a
/// socket, so nothing in this file can construct one - which meant the most
/// important behaviour here, *does pressing a button send its CC*, had no test
/// at all. Deleting the dispatch entirely left every test green when this was
/// checked by mutation on 2026-09-05.
fn button_message(osc_button: &str, is_down: bool) -> Option<Message> {
    panel_button_cc(osc_button).map(|cc| Message::RPN7(Ch1, cc, cc_math::button_cc_value(is_down)))
}

fn osc_button_to_btn_map(osc_button: &str) -> Option<MaschineButton> {
    PANEL_BUTTONS.iter().find(|(name, _, _)| *name == osc_button).map(|(_, btn, _)| *btn)
}

fn btn_to_osc_button_map(btn: MaschineButton) -> &'static str {
    // THE PANEL'S OWN BUTTONS COME FROM THE TABLE, so the forward and inverse
    // directions cannot drift apart - they were two hand-written lists that
    // had to agree and nothing checked that they did. A test walks the table
    // and asserts the round trip.
    if let Some((name, _, _)) = PANEL_BUTTONS.iter().find(|(_, b, _)| *b == btn) {
        return name;
    }
    // WHAT IS LEFT IS THE RAW REPORT ROWS - R1..R8, A1..A8, B*, C*, D*, E*,
    // FF* - which are decode-table names rather than things on the panel. They
    // map to themselves, they have no CC, and `read_buttons` handles rows 8 and
    // up directly, so most of them can never reach here at all.
    match btn {
        MaschineButton::R1 => "R1",
        MaschineButton::R2 => "R2",
        MaschineButton::R3 => "R3",
        MaschineButton::R4 => "R4",
        MaschineButton::R5 => "R5",
        MaschineButton::R6 => "R6",
        MaschineButton::R7 => "R7",
        MaschineButton::R8 => "R8",

        MaschineButton::A1 => "A1",
        MaschineButton::A2 => "A2",
        MaschineButton::A3 => "A3",
        MaschineButton::A4 => "A4",
        MaschineButton::A5 => "A5",
        MaschineButton::A6 => "A6",
        MaschineButton::A7 => "A7",
        MaschineButton::A8 => "A8",

        MaschineButton::B1 => "B1",
        MaschineButton::B2 => "B2",
        MaschineButton::B3 => "B3",
        MaschineButton::B4 => "B4",
        MaschineButton::B5 => "B5",
        MaschineButton::B6 => "B6",
        MaschineButton::B7 => "B7",
        MaschineButton::B8 => "B8",

        MaschineButton::C1 => "C1",
        MaschineButton::C2 => "C2",
        MaschineButton::C3 => "C3",
        MaschineButton::C4 => "C4",
        MaschineButton::C5 => "C5",
        MaschineButton::C6 => "C6",
        MaschineButton::C7 => "C7",
        MaschineButton::C8 => "C8",

        MaschineButton::D1 => "D1",
        MaschineButton::D2 => "D2",
        MaschineButton::D3 => "D3",
        MaschineButton::D4 => "D4",
        MaschineButton::D5 => "D5",
        MaschineButton::D6 => "D6",
        MaschineButton::D7 => "D7",
        MaschineButton::D8 => "D8",

        MaschineButton::E1 => "E1",
        MaschineButton::E2 => "E2",
        MaschineButton::E3 => "E3",
        MaschineButton::E4 => "E4",
        MaschineButton::E5 => "E5",
        MaschineButton::E6 => "E6",
        MaschineButton::E7 => "E7",
        MaschineButton::E8 => "E8",

        MaschineButton::FF1 => "FF1",
        MaschineButton::FF2 => "FF2",
        MaschineButton::FF3 => "FF3",
        MaschineButton::FF4 => "FF4",
        MaschineButton::FF5 => "FF5",
        MaschineButton::FF6 => "FF6",
        MaschineButton::FF7 => "FF7",
        MaschineButton::FF8 => "FF8",

        MaschineButton::G1 => "G1",
        MaschineButton::G2 => "G2",
        MaschineButton::G3 => "G3",
        MaschineButton::G4 => "G4",
        MaschineButton::G5 => "G5",
        MaschineButton::G6 => "G6",
        MaschineButton::G7 => "G7",
        MaschineButton::G8 => "G8",

        MaschineButton::H1 => "H1",
        MaschineButton::H2 => "H2",
        MaschineButton::H3 => "H3",
        MaschineButton::H4 => "H4",
        MaschineButton::H5 => "H5",
        MaschineButton::H6 => "H6",
        MaschineButton::H7 => "H7",
        MaschineButton::H8 => "H8",

        MaschineButton::I1 => "I1",
        MaschineButton::I2 => "I2",
        MaschineButton::I3 => "I3",
        MaschineButton::I4 => "I4",
        MaschineButton::I5 => "I5",
        MaschineButton::I6 => "I6",
        MaschineButton::I7 => "I7",
        MaschineButton::I8 => "I8",

        MaschineButton::J1 => "J1",
        MaschineButton::J2 => "J2",
        MaschineButton::J3 => "J3",
        MaschineButton::J4 => "J4",
        MaschineButton::J5 => "J5",
        MaschineButton::J6 => "J6",
        MaschineButton::J7 => "J7",
        MaschineButton::J8 => "J8",

        MaschineButton::K1 => "K1",
        MaschineButton::K2 => "K2",
        MaschineButton::K3 => "K3",
        MaschineButton::K4 => "K4",
        MaschineButton::K5 => "K5",
        MaschineButton::K6 => "K6",
        MaschineButton::K7 => "K7",
        MaschineButton::K8 => "K8",

        MaschineButton::L1 => "L1",
        MaschineButton::L2 => "L2",
        MaschineButton::L3 => "L3",
        MaschineButton::L4 => "L4",
        MaschineButton::L5 => "L5",
        MaschineButton::L6 => "L6",
        MaschineButton::L7 => "L7",
        MaschineButton::L8 => "L8",

        MaschineButton::M1 => "M1",
        MaschineButton::M2 => "M2",
        MaschineButton::M3 => "M3",
        MaschineButton::M4 => "M4",
        MaschineButton::M5 => "M5",
        MaschineButton::M6 => "M6",
        MaschineButton::M7 => "M7",
        MaschineButton::M8 => "M8",

        MaschineButton::N1 => "N1",
        MaschineButton::N2 => "N2",
        MaschineButton::N3 => "N3",
        MaschineButton::N4 => "N4",
        MaschineButton::N5 => "N5",
        MaschineButton::N6 => "N6",
        MaschineButton::N7 => "N7",
        MaschineButton::N8 => "N8",

        MaschineButton::O1 => "O1",
        MaschineButton::O2 => "O2",
        MaschineButton::O3 => "O3",
        MaschineButton::O4 => "O4",
        MaschineButton::O5 => "O5",
        MaschineButton::O6 => "O6",
        MaschineButton::O7 => "O7",
        MaschineButton::O8 => "O8",

        MaschineButton::P1 => "P1",
        MaschineButton::P2 => "P2",
        MaschineButton::P3 => "P3",
        MaschineButton::P4 => "P4",
        MaschineButton::P5 => "P5",
        MaschineButton::P6 => "P6",
        MaschineButton::P7 => "P7",
        MaschineButton::P8 => "P8",

        _ => "NO",
    }
}

impl<'a> MHandler<'a> {
    fn send_midi(&self, msg: &Message) {
        if let Err(err) = self.seq_port.send_message(msg) {
            println!("midi send failed: {:?}", err);
        }
        self.seq_handle.drain_output();
    }

    fn send_drum(&self, msg: &Message) {
        if let Err(err) = self.drum_port.send_message(msg) {
            println!("drum MIDI send failed: {:?}", err);
        }
        self.seq_handle.drain_output();
    }

    fn release_pads(&mut self, dev: &mut dyn Maschine) {
        for pad in 0..16 {
            if self.pad_note_down[pad] {
                self.send_drum(&Message::NoteOff(Ch1, dev.get_midi_note_base() + self.pad_notes[pad], 0));
                self.pad_note_down[pad] = false;
            }
            if self.pad_held[pad] {
                dev.set_pad_light(pad, self.pad_colors[pad], 1.0);
                self.pad_held[pad] = false;
            }
        }
        self.pad_modifiers = 0;
        dev.set_mod(0);
        for (id, button) in DECKARD_BUTTONS.iter().enumerate() {
            if self.button_held[id] {
                let (color, brightness) = self.button_colors[id];
                dev.set_button_light(*button, color, brightness);
                self.button_held[id] = false;
            }
        }
        // A button release may have happened while USB was unplugged.
        for cc in [2, 10, 25, 26, 29, 30, 31, 33, 49] {
            self.send_midi(&Message::RPN7(Ch1, cc, 0));
        }
    }

    fn pad_color(&self) -> u32 {
        let (r, g, b) = self.color.to_rgb();

        ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    }

    fn pressure_to_vel(&self, pressure: f32) -> U7 {
        (match self.pressure_shape {
            PressureShape::Linear => pressure,
            PressureShape::Exponential(power) => pressure.powf(power),
            PressureShape::Constant(c_pressure) => c_pressure,
        } * 127.0) as U7
    }

    fn recv_osc_msg(&self, maschine: &mut dyn Maschine) {
        let mut buf = [0u8; 128];

        let nbytes = match self.osc_socket.recv_from(&mut buf) {
            Ok((nbytes, _)) => nbytes,
            Err(e) => {
                println!(" :: error in recv_from(): {}", e);
                return;
            }
        };

        let msg = match osc::Message::deserialize(&buf[..nbytes]) {
            Ok(msg) => msg,
            Err(_) => {
                println!(" :: couldn't decode OSC message :c");
                return;
            }
        };

        self.handle_osc_messge(maschine, &msg);
    }

    fn handle_osc_messge(&self, maschine: &mut dyn Maschine, msg: &osc::Message) {
        if msg.path.starts_with("/maschine/button") {
            // strip_prefix, NOT a fixed slice. `&msg.path[17..]` panicked on
            // the exact path "/maschine/button", which is sixteen bytes: the
            // starts_with passed and the slice ran off the end. One OSC
            // message from any local process ended the daemon.
            let name = match msg.path.strip_prefix("/maschine/button/") {
                Some(name) => name,
                None => return,
            };
            let btn = match osc_button_to_btn_map(name) {
                Some(btn) => btn,
                None => return,
            };

            match msg.arguments.len() {
                1 => maschine.set_button_light(
                    btn,
                    0xFFFFFF,
                    match msg.arguments[0] {
                        osc::Argument::i(val) => val as f32,
                        osc::Argument::f(val) => val,
                        _ => return,
                    },
                ),

                2 => {
                    if let (&osc::Argument::i(color), &osc::Argument::f(brightness)) = (&msg.arguments[0], &msg.arguments[1]) {
                        maschine.set_button_light(btn, (color as u32) & 0xFFFFFF, brightness);
                    }
                }

                _ => return,
            };
        } else if msg.path.starts_with("/maschine/pad") {
            match msg.arguments.len() {
                3 => {
                    if let (&osc::Argument::i(pad), &osc::Argument::i(color), &osc::Argument::f(brightness)) =
                        (&msg.arguments[0], &msg.arguments[1], &msg.arguments[2])
                    {
                        // Range-checked BEFORE the cast: a negative i32 becomes
                        // an enormous usize, and set_pad_light indexes a
                        // sixteen-entry table with it.
                        if (0..16).contains(&pad) {
                            maschine.set_pad_light(pad as usize, (color as u32) & 0xFFFFFF, brightness as f32);
                        }
                    }
                }

                _ => return,
            }
        } else if msg.path.starts_with("/maschine/display/test") {
            if let [osc::Argument::i(pattern)] = msg.arguments[..] {
                if pattern >= 0 {
                    maschine.display_test(pattern as usize);
                }
            }
        } else if msg.path.starts_with("/maschine/display/opts") {
            if let [osc::Argument::i(col), osc::Argument::i(reverse), osc::Argument::i(bands)] = msg.arguments[..] {
                if (0..=255).contains(&col) && bands >= 1 {
                    maschine.display_opts(col as u8, reverse != 0, bands as usize);
                }
            }
        } else if msg.path.starts_with("/maschine/display/calib") {
            if let [osc::Argument::i(on)] = msg.arguments[..] {
                maschine.calib_set(on != 0);
            }
        } else if msg.path.starts_with("/maschine/display/fbclear") {
            // Screen framebuffer commands. Screen 0 = left (0xE0), 1 = right
            // (0xE1). Nothing reaches the hardware until the display timer
            // flushes, so a whole screen is composed with several messages and
            // shown in one write.
            if let [osc::Argument::i(screen)] = msg.arguments[..] {
                if screen >= 0 {
                    maschine.display_fb_clear(screen as usize);
                }
            }
        } else if msg.path.starts_with("/maschine/display/raw") {
            if let [osc::Argument::i(on)] = msg.arguments[..] {
                maschine.display_fb_raw(on != 0);
            }
        } else if msg.path.starts_with("/maschine/display/text") {
            if let [osc::Argument::i(screen), osc::Argument::i(x), osc::Argument::i(y), osc::Argument::i(scale), osc::Argument::i(invert), osc::Argument::s(text)] =
                msg.arguments[..]
            {
                if screen >= 0 && x >= 0 && y >= 0 && scale >= 1 {
                    maschine.display_fb_text(screen as usize, x as usize, y as usize, scale as usize, invert != 0, text);
                }
            }
        } else if msg.path.starts_with("/maschine/display/rect") {
            if let [osc::Argument::i(screen), osc::Argument::i(x), osc::Argument::i(y), osc::Argument::i(w), osc::Argument::i(h), osc::Argument::i(style)] =
                msg.arguments[..]
            {
                if screen >= 0 && x >= 0 && y >= 0 && w >= 0 && h >= 0 && style >= 0 {
                    maschine.display_fb_rect(screen as usize, x as usize, y as usize, w as usize, h as usize, style as usize);
                }
            }
        } else if msg.path.starts_with("/maschine/display/clear") {
            maschine.clear_screen();
        } else if msg.path.starts_with("/maschine/rawled") {
            // Diagnostic path for mapping LED report layouts: buffer, index,
            // value. See Maschine::set_raw_light.
            if let [osc::Argument::i(buffer), osc::Argument::i(index), osc::Argument::i(value)] = msg.arguments[..] {
                if buffer >= 0 && index >= 0 && (0..=255).contains(&value) {
                    maschine.set_raw_light(buffer as usize, index as usize, value as u8);
                }
            }
        } else if msg.path.starts_with("/maschine/encoder") {
            // Re-centre an encoder: idx 0-7, value 0-127. A host that points
            // one endless knob at several parameters uses this when it
            // switches between them, so the knob is never parked against an
            // end stop belonging to a parameter it no longer controls.
            if let [osc::Argument::i(idx), osc::Argument::i(value)] = msg.arguments[..] {
                if (0..8).contains(&idx) && (0..=127).contains(&value) {
                    maschine.set_roller_value(value, idx as usize);
                }
            }
        } else if msg.path.starts_with("/maschine/midi_note_base") {
            match msg.arguments.len() {
                1 => {
                    if let osc::Argument::i(base) = msg.arguments[0] {
                        maschine.set_midi_note_base(base as u8);
                    }
                }
                _ => return,
            }
        }
    }

    fn send_osc_msg(&self, path: &str, arguments: Vec<osc::Argument>) {
        let msg = osc::Message { path: path, arguments: arguments };

        let bytes = match msg.serialize() {
            Ok(bytes) => bytes,
            Err(e) => {
                println!(" :: could not serialise OSC {}: {:?}", path, e);
                return;
            }
        };
        match self.osc_socket.send_to(&bytes, &self.osc_outgoing_addr) {
            Ok(_) => {}
            Err(e) => println!(" :: error in send_to: {}", e),
        }
    }

    //Status is Byte from previous stupid naming!
    fn send_osc_button_msg(&mut self, maschine: &mut dyn Maschine, btn: MaschineButton, status: usize, is_down: bool) {
        let modifier = pad_modifier(btn);
        if is_down {
            self.pad_modifiers |= modifier;
        } else {
            self.pad_modifiers &= !modifier;
        }
        if is_down {
            match btn {
                MaschineButton::Pattern => self.pad_mode = 2,
                MaschineButton::Scene => self.pad_mode = 3,
                MaschineButton::Padmode => self.pad_mode = 0,
                _ => {}
            }
        }
        let button = btn_to_osc_button_map(btn);
        let controlbase = 15;
        //println!("{} is:  {}", button, status);
        // EXACT MATCHES, not substrings, since 2026-09-03. This block used to
        // read `button.contains("shift")` and `button.contains("C")` and then
        // test the exact name inside, so the outer guards did nothing except
        // make it look as though a whole family of tokens was handled. They
        // were also a live trap: every uppercase group letter is a single
        // character, so any future token carrying a capital C, E, G, I, K, M or
        // O would have entered a branch that reparks an encoder. The names
        // matched here are exactly the names the old code could reach.
        if button == "shift" {
            maschine.set_mod(if is_down { 1 } else { 0 });
        }
        // The high-order counter bytes are NOT handled here either, for the
        // same reason: "C7", "E8" and the rest are row-8-and-up names, and
        // `read_buttons` sets those directly with `set_roller_state`. Removed
        // 2026-09-03; row 7 - A1..A8, the big encoder's nibble byte - IS a
        // button row and is handled below.
        if button == "A8" {
            // THE BIG ENCODER, and it is the only one that does not go through
            // send_encoder_cc. It used to send `status * 8` as an ABSOLUTE
            // value on every detent, so the counter rolling over snapped the
            // target from 120 to 0 once per revolution - MOD depth rides this
            // knob, so that was audible.
            //
            // Slot 8 of roller_status/roller_value has been sitting free since
            // both arrays were declared [_; 9] for eight encoders. This is
            // what it was for.
            // THE LOW NIBBLE, AND ONLY THE LOW NIBBLE, since 2026-09-02.
            // The HID descriptor declares this byte as `2 x 4 bit, Logical
            // Maximum (15)` - two counters sharing one byte - and the capture
            // of 2026-08-31 settled which one moves: the LOW nibble ran all
            // sixteen values across 117 transitions, every one +1, wrapping
            // 15 to 0, while the HIGH nibble stayed at zero throughout.
            //
            // Reading the whole byte therefore WORKED BY ACCIDENT. It worked
            // because the other nibble happens to be zero, and it would have
            // stopped the moment whatever that nibble counts started moving -
            // the value would jump by 16 per foreign tick and big_encoder_value
            // would read it as a wild turn.
            //
            // notes/findings/2026-08-31-three-hardware-answers-one-capture.md
            let counter = (status & 0x0F) as i32;
            let prev = maschine.get_roller_status(BIG_ENC_SLOT);
            // Resync the baseline whether or not the report is used, exactly
            // as send_encoder_cc does: leaving a stale baseline makes every
            // later delta carry the wrap too, and the knob goes dead until the
            // counter comes back round.
            maschine.set_roller_status(counter, BIG_ENC_SLOT);
            let value = cc_math::big_encoder_value(prev, counter, maschine.get_roller_value(BIG_ENC_SLOT));
            maschine.set_roller_value(value as i32, BIG_ENC_SLOT);
            let msg = Message::RPN7(Ch1, controlbase, value);
            self.send_midi(&msg);
        }

        if status <= 250 {
            // THE PLAIN BUTTONS ARE ONE LINE NOW, not forty-one identical
            // match arms differing only in a number. The number lives in
            // PANEL_BUTTONS, where it can be found.
            if let Some(msg) = button_message(button, is_down) {
                self.send_midi(&msg);
            }
            // WHAT IS LEFT DOES MORE THAN SEND ONE CC. Each of these carries
            // real behaviour beside the message, which is exactly why it is
            // not a table row: PAD MODE has a separate arm, and Group buttons
            // move the pad note base as well as sending their CC.
            match button {
                "pad_mode" => {
                    // Let Deckard receive the button even while SHIFT is held.
                    let msg = Message::RPN7(Ch1, 27, cc_math::button_cc_value(is_down));
                    self.send_midi(&msg);
                }
                // SHIFT is also forwarded to the host as a normal button.

                // NO ENCODER ARMS HERE. There used to be eight - "B6" through
                // "P6", each rebuilding an encoder's absolute position and
                // sending it on CC 16-23 - and they were UNREACHABLE, which
                // the button-name tests at the bottom of this file found on
                // 2026-09-03. `read_buttons` hands rows 0-7 of the report to
                // `button_down` and rows 8-23 to `encoder_step` and
                // `set_roller_state`; every one of those arms was keyed on a
                // row-8-and-up name. The live path is `send_encoder_cc`, and
                // it is the only one that has run for years.
                "group_a" => {
                    let msg = Message::RPN7(Ch1, cc_math::group_cc(0), cc_math::button_cc_value(is_down));
                    if let Err(err) = self.seq_port.send_message(&msg) {
                        println!("group button: MIDI send failed: {:?}", err);
                    }
                    self.seq_handle.drain_output();
                    maschine.set_midi_note_base(36);
                }
                "group_b" => {
                    let msg = Message::RPN7(Ch1, cc_math::group_cc(1), cc_math::button_cc_value(is_down));
                    if let Err(err) = self.seq_port.send_message(&msg) {
                        println!("group button: MIDI send failed: {:?}", err);
                    }
                    self.seq_handle.drain_output();
                    maschine.set_midi_note_base(36);
                }
                "group_c" => {
                    let msg = Message::RPN7(Ch1, cc_math::group_cc(2), cc_math::button_cc_value(is_down));
                    if let Err(err) = self.seq_port.send_message(&msg) {
                        println!("group button: MIDI send failed: {:?}", err);
                    }
                    self.seq_handle.drain_output();
                    maschine.set_midi_note_base(36);
                }
                "group_d" => {
                    let msg = Message::RPN7(Ch1, cc_math::group_cc(3), cc_math::button_cc_value(is_down));
                    if let Err(err) = self.seq_port.send_message(&msg) {
                        println!("group button: MIDI send failed: {:?}", err);
                    }
                    self.seq_handle.drain_output();
                    maschine.set_midi_note_base(36);
                }
                "group_e" => {
                    let msg = Message::RPN7(Ch1, cc_math::group_cc(4), cc_math::button_cc_value(is_down));
                    if let Err(err) = self.seq_port.send_message(&msg) {
                        println!("group button: MIDI send failed: {:?}", err);
                    }
                    self.seq_handle.drain_output();
                    maschine.set_midi_note_base(36);
                }
                "group_f" => {
                    let msg = Message::RPN7(Ch1, cc_math::group_cc(5), cc_math::button_cc_value(is_down));
                    if let Err(err) = self.seq_port.send_message(&msg) {
                        println!("group button: MIDI send failed: {:?}", err);
                    }
                    self.seq_handle.drain_output();
                    maschine.set_midi_note_base(36);
                }
                "group_g" => {
                    let msg = Message::RPN7(Ch1, cc_math::group_cc(6), cc_math::button_cc_value(is_down));
                    if let Err(err) = self.seq_port.send_message(&msg) {
                        println!("group button: MIDI send failed: {:?}", err);
                    }
                    self.seq_handle.drain_output();
                    maschine.set_midi_note_base(36);
                }
                "group_h" => {
                    let msg = Message::RPN7(Ch1, cc_math::group_cc(7), cc_math::button_cc_value(is_down));
                    if let Err(err) = self.seq_port.send_message(&msg) {
                        println!("group button: MIDI send failed: {:?}", err);
                    }
                    self.seq_handle.drain_output();
                    maschine.set_midi_note_base(36);
                }
                _ => {}
            }
        }
        if is_down {
        } else {
        }
        self.send_osc_msg(&*format!("/{}", button), osc_args![status as f32]);
    }

    fn send_encoder_cc(&self, maschine: &mut dyn Maschine, idx: usize, raw: i32) {
        let state = maschine.get_roller_state(idx);
        let accumulated = cc_math::accumulate_raw(raw, state);
        let prev = maschine.get_roller_status(idx);
        let delta = accumulated - prev;
        // A wrap of the hardware counter shows up as a jump far larger than
        // any real movement, so it is not reported. The baseline still has to
        // be resynced to it: leaving the old one in place makes every later
        // delta measure the wrap's size as well, so every one of them is
        // rejected too and the encoder goes dead until the counter comes back
        // round. One wrap should cost one message, not the knob.
        maschine.set_roller_status(accumulated, idx);
        if delta != 0 && !cc_math::is_encoder_jump(delta) {
            // An endless encoder must not get stuck at MIDI 0 or 127.
            let value = wrap_encoder(maschine.get_roller_value(idx), delta);
            let cc_num = self.encoder_ccs[idx];
            let msg = Message::RPN7(Ch1, cc_num, value);
            maschine.set_roller_value(value as i32, idx);
            self.send_midi(&msg);
        }
    }
}

impl<'a> MaschineHandler for MHandler<'a> {
    fn pad_pressed(&mut self, maschine: &mut dyn Maschine, pad_idx: usize, pressure: f32) {
        let midi_note = maschine.get_midi_note_base() + self.pad_notes[pad_idx];
        let msg = Message::NoteOn(Ch1, midi_note, self.pressure_to_vel(pressure));
        self.send_midi(&msg);
        if self.pad_modifiers == 0 && self.pad_mode == 0 {
            self.send_drum(&msg);
            self.pad_note_down[pad_idx] = true;
        }
        self.pad_held[pad_idx] = true;
        // Show the touch immediately using Deckard's last colour; the app's
        // next state update remains authoritative for the base colour.
        maschine.set_pad_light(pad_idx, pressed_pad_color(self.pad_colors[pad_idx]), 1.0);
        if !self.external_pad_leds {
            maschine.set_pad_light(pad_idx, self.pad_color(), pressure.sqrt());
        }
    }

    fn pad_aftertouch(&mut self, maschine: &mut dyn Maschine, pad_idx: usize, pressure: f32) {
        match self.pressure_shape {
            PressureShape::Constant(_) => return,
            _ => {}
        }

        if !self.send_aftertouch {
            return;
        }

        // Two gates, and the LED glow sits behind them too. Without that the
        // pressure glow would mark the LED buffer dirty on every report and
        // keep the LED flush at its maximum rate while a pad was held.
        // AFTERTOUCH ONLY. The raw pressure is normalised against 4095 in
        // mikro.rs, and all sixteen pads were measured on 2026-08-31 topping
        // out between 3101 and 3479 - so a fifth of the control was
        // unreachable however hard anyone pressed. Rescaled here rather than
        // at the divisor, because that divisor also feeds NOTE VELOCITY, which
        // is shipped and gated. Owner's call: scale aftertouch, leave velocity
        // alone.
        let value = self.pressure_to_vel(cc_math::aftertouch_scale(pressure));
        if value == self.at_last_val[pad_idx] {
            return;
        }
        if let Some(sent) = self.at_last_sent[pad_idx] {
            if sent.elapsed() < AFTERTOUCH_MIN_INTERVAL {
                return;
            }
        }
        self.at_last_val[pad_idx] = value;
        self.at_last_sent[pad_idx] = Some(Instant::now());

        let midi_note = maschine.get_midi_note_base() + self.pad_notes[pad_idx];
        let msg = Message::PolyphonicPressure(Ch1, midi_note, value);

        if self.pad_note_down[pad_idx] {
            self.send_drum(&msg);
        }

        if !self.external_pad_leds {
            maschine.set_pad_light(pad_idx, self.pad_color(), pressure.sqrt());
        }
    }

    fn pad_released(&mut self, maschine: &mut dyn Maschine, pad_idx: usize) {
        // Re-arm the aftertouch gates, or the next press of this pad is held
        // off by up to 25 ms and its first value can be swallowed as
        // unchanged.
        self.at_last_val[pad_idx] = 0;
        self.at_last_sent[pad_idx] = None;

        let midi_note = maschine.get_midi_note_base() + self.pad_notes[pad_idx];
        let msg = Message::NoteOff(Ch1, midi_note, 0);
        self.send_midi(&msg);
        if self.pad_note_down[pad_idx] {
            self.send_drum(&msg);
            self.pad_note_down[pad_idx] = false;
        }
        self.pad_held[pad_idx] = false;
        maschine.set_pad_light(pad_idx, self.pad_colors[pad_idx], 1.0);
        if !self.external_pad_leds {
            maschine.set_pad_light(pad_idx, self.pad_color(), PAD_RELEASED_BRIGHTNESS);
        }
    }

    fn encoder_step(&mut self, maschine: &mut dyn Maschine, idx: usize, state: i32) {
        if maschine.calib_active() {
            maschine.calib_move(idx, state);
            return;
        }
        self.send_encoder_cc(maschine, idx, state);
    }

    fn button_down(&mut self, maschine: &mut dyn Maschine, btn: MaschineButton, byte: u8, is_down: bool) {
        if let Some(id) = DECKARD_BUTTONS.iter().position(|button| *button == btn).filter(|&id| id != 18) {
            self.button_held[id] = true;
            let (color, brightness) = self.button_colors[id];
            let (color, brightness) = button_shown(id, color, brightness, true);
            maschine.set_button_light(btn, color, brightness);
        }
        //println!("{}", byte);
        self.send_osc_button_msg(maschine, btn, byte as usize, is_down);
    }

    fn button_up(&mut self, maschine: &mut dyn Maschine, btn: MaschineButton, byte: u8, is_down: bool) {
        if let Some(id) = DECKARD_BUTTONS.iter().position(|button| *button == btn).filter(|&id| id != 18) {
            self.button_held[id] = false;
            let (color, brightness) = self.button_colors[id];
            maschine.set_button_light(btn, color, brightness);
        }
        self.send_osc_button_msg(maschine, btn, byte as usize, is_down);
    }

    fn midi_in_received(&mut self, _maschine: &mut dyn Maschine, bytes: &[u8]) {
        for msg in midi_parse::parse(bytes) {
            let _ = self.seq_port.send_message(&msg);
        }
        self.seq_handle.drain_output();
    }
}

fn main() {
    let args: Vec<_> = env::args().collect();

    if args.len() < 2 {
        usage(&args[0]);
        panic!("missing hidraw device path");
    }

    let dev_fd = match fcntl::open(Path::new(&args[1]), OFlag::O_RDWR | OFlag::O_NONBLOCK, sys::stat::Mode::empty()) {
        Err(err) => panic!("couldn't open {}: {}", args[1], err.desc()),
        Ok(file) => file,
    };

    let osc_socket = match UdpSocket::bind("127.0.0.1:42434") {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to bind OSC socket on port 42434: {}", e);
            eprintln!("hint: is another maschine daemon already running?");
            std::process::exit(1);
        }
    };

    let cfg = MaschineConfig::load();

    // A new client name on bridge restart makes Deckard reopen its MIDI
    // subscriptions even if ALSA reuses the same numeric client id.
    let seq_name = format!("maschine.rs {}", std::process::id());
    let seq_handle = SequencerHandle::open(&seq_name, HandleOpenStreams::Duplex).unwrap_or_else(|_| {
        eprintln!("warning: ALSA not available — MIDI disabled");
        SequencerHandle::null()
    });
    if let Err(error) = seq_handle.set_input_pool(1024) {
        panic!("could not allocate the MIDI input pool: {:?}", error);
    }

    let drum_port = seq_handle
        .create_port(
            "Pads MIDI",
            PortCapabilities::PORT_CAPABILITY_READ | PortCapabilities::PORT_CAPABILITY_SUBS_READ,
            PortType::MidiGeneric,
        )
        .unwrap();
    let seq_port = seq_handle
        .create_port(
            "Controls MIDI",
            PortCapabilities::PORT_CAPABILITY_READ
                | PortCapabilities::PORT_CAPABILITY_SUBS_READ
                | PortCapabilities::PORT_CAPABILITY_WRITE
                | PortCapabilities::PORT_CAPABILITY_SUBS_WRITE,
            PortType::MidiGeneric,
        )
        .unwrap();
    let _seq_in_port = seq_handle
        .create_port(
            "MIDI Control",
            PortCapabilities::PORT_CAPABILITY_WRITE | PortCapabilities::PORT_CAPABILITY_SUBS_WRITE,
            PortType::MidiGeneric,
        )
        .unwrap();
    seq_handle.set_nonblock();
    let seq_in_fd = seq_handle.get_poll_fd();

    let mut dev = devices::mk2::Mikro::new(dev_fd);

    let mut handler = MHandler {
        color: HSL { h: 0.0, s: 1.0, l: 0.3 },

        seq_port: &seq_port,
        drum_port: &drum_port,
        seq_handle: &seq_handle,

        pressure_shape: PressureShape::Exponential(0.4),
        send_aftertouch: cfg.send_aftertouch,

        osc_socket: &osc_socket,
        osc_outgoing_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 42435)),

        seq_in_fd,

        pad_notes: cfg.pad_notes,
        encoder_ccs: cfg.encoder_ccs,
        // In the Deckard integration the app owns the pad colours.
        external_pad_leds: true,
        pad_modifiers: 0,
        pad_mode: 0,
        pad_note_down: [false; 16],
        pad_colors: [0; 16],
        pad_held: [false; 16],
        button_colors: [(0, 0.0); 47],
        button_held: [false; 47],
        screen_lines: std::array::from_fn(|_| std::array::from_fn(|_| String::new())),
        display_transport: display_transport::Transport::default(),

        at_last_val: [0; 16],
        at_last_sent: [None; 16],
    };

    // Panel brightness and contrast, once, before anything else touches the
    // device. A no-op unless maschine.json carries both keys; see
    // hid_feature.rs for why the read comes first and why the write is
    // skipped when the device already agrees.
    dev.apply_screen_settings(cfg.screen_brightness, cfg.screen_contrast);

    // Display disabled, see write_display() above.
    // dev.clear_screen();

    if !handler.external_pad_leds {
        for i in 0..16 {
            dev.set_pad_light(i, handler.pad_color(), PAD_RELEASED_BRIGHTNESS);
        }
    }

    ev_loop(&mut dev, &mut handler, &args[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::mk2::BUTTON_REPORT_TO_MIKROBUTTONS_MAP;

    #[test]
    fn deckard_led_feedback_is_scoped_and_range_checked() {
        assert_eq!(deckard_button_led(&[0xf0, 0x7d, 0x44, 0x4b, 0, 127, 0, 0, 0xf7]), Some((MaschineButton::GroupA, 0xff0000, 1.0)));
        assert_eq!(deckard_button_led(&[0xf0, 0x7d, 0x44, 0x4b, 8, 64, 64, 64, 0xf7]).map(|v| v.0), Some(MaschineButton::Play));
        assert!(deckard_button_led(&[0xf0, 0x7d, 0x44, 0x4b, 127, 0, 0, 0, 0xf7]).is_none());
        assert!(deckard_button_led(&[0xf0, 0x7d, 0x44, 0x4b, 0, 255, 0, 0, 0xf7]).is_none());
        assert!(deckard_button_led(&[0xf0, 0x7d, 0x45, 0x4b, 0, 0, 0, 0, 0xf7]).is_none());
    }

    #[test]
    fn deckard_pad_rgb_has_a_separate_bounded_range() {
        assert_eq!(deckard_pad_led(&[0xf0, 0x7d, 0x44, 0x4b, 48, 127, 0, 0, 0xf7]), Some((0, 0xff0000)));
        assert_eq!(deckard_pad_led(&[0xf0, 0x7d, 0x44, 0x4b, 63, 0, 127, 0, 0xf7]), Some((15, 0x00ff00)));
        assert!(deckard_pad_led(&[0xf0, 0x7d, 0x44, 0x4b, 47, 0, 0, 0, 0xf7]).is_none());
        assert!(deckard_pad_led(&[0xf0, 0x7d, 0x44, 0x4b, 64, 0, 0, 0, 0xf7]).is_none());
        assert!(deckard_pad_led(&[0xf0, 0x7d, 0x44, 0x4b, 48, 128, 0, 0, 0xf7]).is_none());
    }

    #[test]
    fn endless_knobs_wrap_and_pad_modifiers_are_distinct() {
        assert_eq!(wrap_encoder(127, 2), 1);
        assert_eq!(wrap_encoder(0, -1), 127);
        assert_eq!(pad_modifier(MaschineButton::Mute), 1);
        assert_eq!(pad_modifier(MaschineButton::Solo), 2);
        assert_eq!(pad_modifier(MaschineButton::Select), 4);
        assert_eq!(pad_modifier(MaschineButton::Scene), 8);
        assert_eq!(pad_modifier(MaschineButton::Pattern), 16);
        assert_eq!(pad_modifier(MaschineButton::Duplicate), 32);
        assert_eq!(pad_modifier(MaschineButton::Erase), 64);
        assert_eq!(pad_modifier(MaschineButton::Play), 0);
    }

    #[test]
    fn secondary_group_ids_are_bounded_and_never_alias_pad_mode() {
        for (id, start) in (65..=72).zip([4, 10, 16, 19, 28, 31, 40, 43]) {
            assert_eq!(deckard_group_indicator(&[0xf0, 0x7d, 0x44, 0x4b, id, 127, 0, 64, 0xf7]), Some((start, [255, 0, 128])));
        }
        for id in [0, 7, 47, 48, 63, 64, 73, 127] {
            assert!(deckard_group_indicator(&[0xf0, 0x7d, 0x44, 0x4b, id, 0, 0, 0, 0xf7]).is_none());
        }
        assert!(deckard_group_indicator(&[0xf0, 0x7d, 0x44, 0x4b, 65, 128, 0, 0, 0xf7]).is_none());
    }

    #[test]
    fn screen_lines_are_bounded_printable_and_addressed() {
        assert_eq!(deckard_screen_line(&[0xf0, 0x7d, 0x44, 0x53, 1, 7, b'O', b'K', 0xf7]), Some((1, 7, "OK")));
        assert!(deckard_screen_line(&[0xf0, 0x7d, 0x44, 0x53, 2, 0, 0xf7]).is_none());
        assert!(deckard_screen_line(&[0xf0, 0x7d, 0x44, 0x53, 0, 8, 0xf7]).is_none());
        assert!(deckard_screen_line(&[0xf0, 0x7d, 0x44, 0x53, 0, 0, 0x01, 0xf7]).is_none());
    }

    #[test]
    fn screen_graphics_decode_primitives_without_pixel_payloads() {
        assert_eq!(deckard_screen_graphic(&[0xf0, 0x7d, 0x44, 0x47, 0, 1, 0xf7]), Some(ScreenGraphic::Clear { screen: 1 }));
        assert_eq!(
            deckard_screen_graphic(&[0xf0, 0x7d, 0x44, 0x47, 1, 0, 4, 0, 8, 0, 32, 0, 8, 0, 1, 0xf7]),
            Some(ScreenGraphic::Rect { screen: 0, x: 4, y: 8, w: 32, h: 8, style: 1 })
        );
        assert!(deckard_screen_graphic(&[0xf0, 0x7d, 0x44, 0x47, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xf7]).is_none());
        assert!(deckard_screen_graphic(&[0xf0, 0x7d, 0x44, 0x47, 1, 0, 4, 0, 8, 0, 32, 0, 8, 0, 6, 0xf7]).is_none());
    }
    #[test]
    fn waveform_packets_bound_the_columns_geometry_and_flags() {
        let valid = [0xf0, 0x7d, 0x44, 0x47, 3, 1, 3, 0, 22, 29, 2, 16, 1, 2, 0xf7];
        assert_eq!(
            deckard_screen_graphic(&valid),
            Some(ScreenGraphic::WaveColumns { screen: 1, x: 3, y: 22, h: 29, columns: &[2, 16, 1, 2] })
        );
        for (at, value) in [(5, 2), (6, 127), (7, 2), (8, 40), (9, 2), (10, 17), (11, 29), (13, 4)] {
            let mut invalid = valid;
            invalid[at] = value;
            // x=127 is valid for one column, so extend it to the right edge.
            if at == 6 {
                invalid[7] = 1;
            }
            assert!(deckard_screen_graphic(&invalid).is_none(), "accepted invalid field {at}");
        }
        let mut full = vec![0xf0, 0x7d, 0x44, 0x47, 3, 0, 0, 0, 0, 32];
        for _ in 0..13 {
            full.extend_from_slice(&[0, 31, 127, 3]);
        }
        full.push(0xf7);
        assert!(deckard_screen_graphic(&full).is_some());
        full.splice(10..10, [0, 0, 0, 0]);
        assert!(deckard_screen_graphic(&full).is_none());
    }

    #[test]
    fn graphic_text_uses_pixel_coordinates_and_validates_the_payload() {
        assert_eq!(
            deckard_screen_graphic(&[0xf0, 0x7d, 0x44, 0x47, 2, 1, 64, 1, 22, 0, 2, b'O', b'K', 0xf7]),
            Some(ScreenGraphic::Text { screen: 1, x: 192, y: 22, scale: 2, text: "OK" })
        );
        for (at, value) in [(5, 2), (7, 2), (8, 64), (10, 0), (10, 3), (11, 31), (11, 128)] {
            let mut invalid = [0xf0, 0x7d, 0x44, 0x47, 2, 0, 0, 0, 0, 0, 1, b'A', 0xf7];
            invalid[at] = value;
            assert!(deckard_screen_graphic(&invalid).is_none());
        }
        assert!(deckard_screen_graphic(&[0xf0, 0x7d, 0x44, 0x47, 1, 0, 128, 0, 0, 0, 1, 0, 1, 0, 1, 0xf7]).is_none());
    }

    #[test]
    fn screen_updates_only_changed_character_cells() {
        assert_eq!(changed_cells("same", "same"), None);
        assert_eq!(changed_cells("TUNE 50%", "TUNE 51%"), Some((6, 7)));
        assert_eq!(changed_cells("longer", "long"), Some((4, 6)));
        assert_eq!(changed_cells("A", "AB"), Some((1, 2)));
    }

    #[test]
    fn pad_mode_only_accepts_the_four_supported_modes() {
        assert_eq!(deckard_pad_mode(&[0xf0, 0x7d, 0x44, 0x4b, 64, 1, 0, 0, 0xf7]), Some(1));
        assert_eq!(deckard_pad_mode(&[0xf0, 0x7d, 0x44, 0x4b, 64, 2, 0, 0, 0xf7]), Some(2));
        assert_eq!(deckard_pad_mode(&[0xf0, 0x7d, 0x44, 0x4b, 64, 3, 0, 0, 0xf7]), Some(3));
        assert!(deckard_pad_mode(&[0xf0, 0x7d, 0x44, 0x4b, 64, 4, 0, 0, 0xf7]).is_none());
    }

    #[test]
    fn pad_touch_brightens_the_last_colour_without_losing_rgb() {
        assert_eq!(pressed_pad_color(0), 0x404040);
        assert_eq!(pressed_pad_color(0x204080), 0x4080ff);
        assert_eq!(pressed_pad_color(0xff0000), 0xff0000);
        assert_eq!(pressed_pad_color(0xa3ff18), 0xa3ff18);
        assert_eq!(pressed_pad_color(0x506432), 0xa0c864);
        assert_eq!(button_shown(0, 0x204080, 0.5, true), (0x4080ff, 1.0));
        assert_eq!(button_shown(8, 0, 0.2, true), (0, 1.0));
        assert_eq!(button_shown(8, 0, 0.2, false), (0, 0.2));
    }

    #[test]
    fn modifiers_and_step_mode_suppress_new_notes_but_release_old_ones() {
        let handle = SequencerHandle::null();
        let controls = handle.create_port("controls", PortCapabilities::PORT_CAPABILITY_READ, PortType::MidiGeneric).unwrap();
        let drums = handle.create_port("drums", PortCapabilities::PORT_CAPABILITY_READ, PortType::MidiGeneric).unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut handler = MHandler {
            color: HSL { h: 0.0, s: 1.0, l: 0.3 },
            seq_handle: &handle,
            seq_port: &controls,
            drum_port: &drums,
            pressure_shape: PressureShape::Linear,
            send_aftertouch: false,
            osc_socket: &socket,
            osc_outgoing_addr: "127.0.0.1:42435".parse().unwrap(),
            seq_in_fd: -1,
            pad_notes: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            encoder_ccs: [16, 17, 18, 19, 20, 21, 22, 23],
            external_pad_leds: true,
            pad_modifiers: 0,
            pad_mode: 0,
            pad_note_down: [false; 16],
            pad_colors: [0; 16],
            pad_held: [false; 16],
            button_colors: [(0, 0.0); 47],
            button_held: [false; 47],
            screen_lines: std::array::from_fn(|_| std::array::from_fn(|_| String::new())),
            display_transport: display_transport::Transport::default(),
            at_last_val: [0; 16],
            at_last_sent: [None; 16],
        };
        let mut dev = devices::mk2::Mikro::new(-1);
        handler.pad_pressed(&mut dev, 0, 1.0);
        assert!(handler.pad_note_down[0]);
        assert!(handler.pad_held[0]);
        handler.button_down(&mut dev, MaschineButton::Mute, 1, true);
        assert!(handler.button_held[35]);
        handler.pad_pressed(&mut dev, 1, 1.0);
        assert!(!handler.pad_note_down[1]);
        handler.pad_released(&mut dev, 0);
        assert!(!handler.pad_note_down[0]);
        assert!(!handler.pad_held[0]);
        handler.button_up(&mut dev, MaschineButton::Mute, 0, false);
        assert!(!handler.button_held[35]);
        handler.pad_pressed(&mut dev, 2, 1.0);
        assert!(handler.pad_note_down[2]);
        // STEP toggles FOLLOW in Deckard and keeps the current pad routing.
        handler.button_down(&mut dev, MaschineButton::Step, 1, true);
        assert_eq!(handler.pad_mode, 0);
        handler.pad_pressed(&mut dev, 3, 1.0);
        assert!(handler.pad_note_down[3]);
        handler.button_up(&mut dev, MaschineButton::Step, 0, false);
        handler.button_down(&mut dev, MaschineButton::Pattern, 1, true);
        assert_eq!(handler.pad_mode, 2);
        handler.pad_pressed(&mut dev, 4, 1.0);
        assert!(!handler.pad_note_down[4]);
        handler.button_up(&mut dev, MaschineButton::Pattern, 0, false);
        handler.button_down(&mut dev, MaschineButton::Padmode, 1, true);
        assert_eq!(handler.pad_mode, 0);
        handler.release_pads(&mut dev);
        assert!(!handler.pad_note_down.iter().any(|down| *down));
        assert!(!handler.pad_held.iter().any(|down| *down));
    }

    /// Report rows that `read_buttons` hands to `button_down` / `button_up`.
    ///
    /// Rows 8-23 of the same table are the ENCODER bytes: read_buttons sends
    /// those to `encoder_step` and `set_roller_state` and never to a button
    /// handler, so their names (B1..Q8) never reach the host at all. Walking
    /// them here is what found the eight dead match arms this file used to
    /// carry - and it is why the range stops at 8 rather than at the end.
    ///
    /// Row 7 IS a button row: A1..A8 is byte 7, the big encoder's nibble pair,
    /// which arrives as eight fake buttons and is decoded in "A8".
    const BUTTON_ROWS: usize = 8;

    /// Every button the DEVICE can actually send, taken from the report decode
    /// table rather than from a list written by hand - a fourth list would
    /// drift from the three that already exist.
    fn every_reachable_button() -> Vec<MaschineButton> {
        BUTTON_REPORT_TO_MIKROBUTTONS_MAP[..BUTTON_ROWS].iter().flatten().filter_map(|b| *b).collect()
    }

    #[test]
    fn every_reachable_button_has_a_name() {
        // `btn_to_osc_button_map` ends in `_ => "NO"`, so an unnamed button is
        // not a compile error: it silently becomes a button called NO, and
        // every one of them collides with all the others.
        for btn in every_reachable_button() {
            let name = btn_to_osc_button_map(btn);
            assert_ne!(name, "NO", "{:?} reaches the host as \"NO\"", btn);
        }
    }

    #[test]
    fn no_two_buttons_share_a_name() {
        // THIS IS THE FF7 DEFECT, kept executable. `MaschineButton::FF7`
        // answered to "FF8": two buttons on one token, the seventh
        // unreachable, and nothing could see it because both tables are
        // maintained by hand.
        let mut seen: Vec<(&str, MaschineButton)> = Vec::new();
        for btn in every_reachable_button() {
            let name = btn_to_osc_button_map(btn);
            if let Some((_, other)) = seen.iter().find(|(n, _)| *n == name) {
                panic!("{:?} and {:?} both answer to {:?}", btn, other, name);
            }
            seen.push((name, btn));
        }
    }

    #[test]
    fn the_named_buttons_round_trip() {
        // The reverse table is what the driver's LED writes go through, so a
        // name that does not come back to the same button is an LED that
        // lights the wrong thing - which has happened here twice.
        for btn in every_reachable_button() {
            let name = btn_to_osc_button_map(btn);
            if let Some(back) = osc_button_to_btn_map(name) {
                assert_eq!(btn_to_osc_button_map(back), name, "{:?} -> {:?} -> {:?}", btn, name, back);
            }
        }
    }

    #[test]
    fn the_encoder_arms_agree_with_the_shared_formula() {
        // Eight unreachable arms used to open-code `status / 4 + state * 64`
        // beside a tested function nothing called. One copy now, on the live
        // path, and this pins the two together.
        for raw in [0i32, 4, 100, 252] {
            for state in [0usize, 1, 3] {
                assert_eq!(cc_math::accumulate_raw(raw, state), raw / 4 + state as i32 * 64);
            }
        }
    }
    // --- PANEL_BUTTONS, the one table, 2026-09-05 (todo item 23) ------------

    #[test]
    fn the_name_lookup_round_trips_every_row() {
        // This is the property the two hand-written tables had to have and
        // that nothing checked. They HAPPENED to agree when they were merged -
        // verified row by row at the time - but nothing would have said so.
        for (name, btn, _) in PANEL_BUTTONS {
            assert_eq!(osc_button_to_btn_map(name), Some(*btn), "{} -> btn", name);
            assert_eq!(btn_to_osc_button_map(*btn), *name, "{:?} -> name", btn);
        }
    }

    #[test]
    fn no_name_and_no_variant_appears_twice() {
        // A duplicate name makes the second row unreachable; a duplicate
        // variant makes the INVERSE direction pick whichever comes first,
        // silently. Both are the kind of thing a hand-written pair of tables
        // grows and nobody notices.
        let mut names: Vec<&str> = PANEL_BUTTONS.iter().map(|(n, _, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "a token appears twice in PANEL_BUTTONS");

        for (i, (_, a, _)) in PANEL_BUTTONS.iter().enumerate() {
            for (_, b, _) in PANEL_BUTTONS.iter().skip(i + 1) {
                assert!(a != b, "{:?} appears twice in PANEL_BUTTONS", a);
            }
        }
    }

    #[test]
    fn no_cc_is_used_by_two_buttons() {
        // The CC map is MEASURED HARDWARE FACT. Two buttons sharing a number
        // means the driver cannot tell them apart, and it would look like one
        // of them being dead.
        let mut ccs: Vec<u16> = PANEL_BUTTONS.iter().filter_map(|(_, _, c)| *c).collect();
        ccs.sort_unstable();
        let before = ccs.len();
        ccs.dedup();
        assert_eq!(ccs.len(), before, "two buttons share a CC");
    }

    #[test]
    fn the_buttons_with_no_cc_are_exactly_the_ones_that_do_more() {
        // `None` is not "no CC exists" - it is "the generic sender must not
        // fire, because something below it already handles this token and
        // does more than send one message". If a name joins this list the
        // reason has to be that it grew behaviour, not that its CC went
        // missing: a silently absent CC is a dead button.
        let mut special: Vec<&str> = PANEL_BUTTONS.iter().filter(|(_, _, cc)| cc.is_none()).map(|(n, _, _)| *n).collect();
        special.sort_unstable();
        assert_eq!(
            special,
            vec!["encoder", "group_a", "group_b", "group_c", "group_d", "group_e", "group_f", "group_g", "group_h", "pad_mode"]
        );
    }

    #[test]
    fn every_plain_button_produces_its_message() {
        // THE BEHAVIOUR THIS FILE EXISTS FOR, and until 2026-09-05 nothing
        // tested it: deleting the dispatch left all 177 tests green. Down and
        // up are both asserted, because a button that only sent one edge would
        // latch on the driver's side.
        for (name, _, cc) in PANEL_BUTTONS {
            match cc {
                Some(cc) => {
                    assert_eq!(button_message(name, true), Some(Message::RPN7(Ch1, *cc, cc_math::button_cc_value(true))), "{} down", name);
                    assert_eq!(button_message(name, false), Some(Message::RPN7(Ch1, *cc, cc_math::button_cc_value(false))), "{} up", name);
                }
                None => assert_eq!(button_message(name, true), None, "{}", name),
            }
        }
    }

    #[test]
    fn an_unknown_token_sends_nothing() {
        assert_eq!(button_message("not_a_button", true), None);
    }

    #[test]
    fn the_two_edges_differ() {
        // If press and release carried the same value the driver could not
        // tell a hold from a tap, which is the whole duration rule.
        assert_ne!(button_message("play", true), button_message("play", false));
    }

    #[test]
    fn the_ccs_that_shipped_did_not_move() {
        // A spot check against the MEASURED map, so a refactor of the table
        // cannot quietly renumber the panel. These four are the ones a player
        // would notice first and the ones the guide names.
        assert_eq!(panel_button_cc("play"), Some(1));
        assert_eq!(panel_button_cc("swing"), Some(50)); // MOD lives here
        assert_eq!(panel_button_cc("enter"), Some(36));
        assert_eq!(panel_button_cc("page_left"), Some(48));
        assert_eq!(panel_button_cc("nope"), None);
    }
}
