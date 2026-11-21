"""Exercise service transitions without sudo or changes to real services."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/tty6.fish'

class Tty6Script(unittest.TestCase):
    def invoke(self, action, vt='1', fail=False):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            login = root / 'loginctl'
            login.write_text('#!/bin/sh\ncase "$1" in\nlist-sessions) echo "3 1000 user seat0";;\nshow-session) echo "$MOCK_VT";;\nesac\n')
            sudo = root / 'sudo'
            sudo.write_text('#!/bin/sh\necho "$*" >> "$MOCK_LOG"\nif [ "$MOCK_FAIL" = 1 ] && [ "$*" = "systemctl start getty@tty6.service" ]; then exit 1; fi\n')
            login.chmod(0o755)
            sudo.chmod(0o755)
            log = root / 'calls'
            env = dict(os.environ, PATH=f'{root}:'+os.environ['PATH'], MOCK_LOG=str(log), MOCK_VT=vt, MOCK_FAIL=str(int(fail)))
            result = subprocess.run(['fish', '--no-config', str(SCRIPT), action], env=env, capture_output=True, text=True)
            return result, log.read_text().splitlines() if log.exists() else []

    def test_prepare_blocks_autovt_before_stopping_kmscon(self):
        result, calls = self.invoke('prepare')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ['systemctl mask --runtime kmsconvt@tty6.service autovt@tty6.service', 'systemctl stop kmsconvt@tty6.service', 'systemctl start getty@tty6.service'])

    def test_restore_removes_both_runtime_masks(self):
        result, calls = self.invoke('restore')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ['systemctl unmask --runtime kmsconvt@tty6.service autovt@tty6.service', 'systemctl stop getty@tty6.service', 'systemctl start kmsconvt@tty6.service'])

    def test_vtnr_protects_a_logged_in_kmscon_session(self):
        result, calls = self.invoke('prepare', vt='6')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, [])

    def test_failed_getty_start_rolls_back_masks_and_kmscon(self):
        result, calls = self.invoke('prepare', fail=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls[-2:], ['systemctl unmask --runtime kmsconvt@tty6.service autovt@tty6.service', 'systemctl start kmsconvt@tty6.service'])

if __name__ == '__main__':
    unittest.main()
