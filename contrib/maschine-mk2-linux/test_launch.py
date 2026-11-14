# SPDX-License-Identifier: GPL-3.0-or-later
import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location('maschine_launch', Path(__file__).with_name('launch.py'))
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)


class DeviceLookupTest(unittest.TestCase):
    def test_only_the_connected_mk2_is_selected_and_replug_is_detected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sysfs, dev = root / 'sys', root / 'dev'
            for index, identity in ((1, 'OTHER'), (5, launcher.HID_ID)):
                entry = sysfs / f'hidraw{index}' / 'device'
                entry.mkdir(parents=True)
                (entry / 'uevent').write_text(identity + '\n')
                dev.mkdir(exist_ok=True)
                (dev / f'hidraw{index}').touch()
            by_id = dev / 'input/by-id'
            by_id.mkdir(parents=True)
            link = by_id / 'maschine-test-if02-hidraw'
            link.symlink_to(dev / 'hidraw5')
            node, identity = launcher.find_device(sysfs, dev)
            self.assertEqual(node, link)
            (sysfs / 'hidraw5/device/uevent').write_text('OTHER\n')
            self.assertIsNone(launcher.find_device(sysfs, dev))
            self.assertTrue(identity.exists())

    def test_same_device_replug_keeps_the_bridge_and_alsa_ports(self):
        link = Path('/dev/input/by-id/maschine-test-if02-hidraw')
        bridge = MagicMock()
        bridge.poll.return_value = None
        with tempfile.TemporaryDirectory() as tmp, \
                patch.dict(launcher.os.environ, {'XDG_DATA_HOME': tmp}), \
                patch.object(launcher.sys, 'argv', ['launch.py', '/bridge']), \
                patch.object(launcher.signal, 'signal'), \
                patch.object(launcher, 'find_device', side_effect=[(link, Path('/old')), None, (link, Path('/new'))]), \
                patch.object(launcher.subprocess, 'Popen', return_value=bridge) as start, \
                patch.object(launcher.time, 'sleep', side_effect=[None, None, KeyboardInterrupt]):
            launcher.main()
        start.assert_called_once()
        bridge.terminate.assert_called_once()  # Only the final shutdown, not the unplug.


if __name__ == '__main__':
    unittest.main()
