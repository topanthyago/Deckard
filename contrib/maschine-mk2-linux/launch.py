#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run the MK2 bridge for the connected HID node, including after replugging."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

HID_ID = "HID_ID=0003:000017CC:00001140"


def find_device(sysfs=Path('/sys/class/hidraw'), devices=Path('/dev')):
    for entry in sorted(sysfs.glob('hidraw*')):
        try:
            if HID_ID in (entry / 'device/uevent').read_text().splitlines():
                node = devices / entry.name
                if os.access(node, os.R_OK | os.W_OK):
                    for link in sorted((devices / 'input/by-id').glob('*-if02-hidraw')):
                        if link.resolve() == node.resolve():
                            return link, (entry / 'device').resolve()
        except OSError:
            continue
    return None


def main():
    state = Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local/share')) / 'deckard/maschine-mk2'
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    child = None

    def stop(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        while True:
            device = find_device()
            if device is None:
                time.sleep(2)
                continue
            node, _identity = device
            print('Maschine MK2: starting MIDI bridge', flush=True)
            child = subprocess.Popen([sys.argv[1], str(node)], cwd=state)
            time.sleep(1)
            while child.poll() is None:
                current = find_device()
                # Keep the ALSA ports while this same controller is unplugged:
                # the daemon reopens the stable by-id link when it comes back.
                if current is not None and current[0] != node:
                    break
                time.sleep(1)
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            child = None
            time.sleep(2)
    except KeyboardInterrupt:
        pass
    finally:
        if child and child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


if __name__ == '__main__':
    main()
