"""Stage fish installation into temporary directories; never touch the user's install/config."""
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class Installer(unittest.TestCase):
    def test_user_install_and_reinstall_preserve_config_and_directory_mode(self):
        with tempfile.TemporaryDirectory(prefix='gc install ') as temp:
            root = Path(temp)
            prefix = root / 'a prefix'
            config_dir = root / 'user config'
            config_dir.mkdir(mode=0o700)
            command = ['fish', '--no-config', str(ROOT / 'install.fish'), '--no-build', '--prefix', str(prefix), '--config-dir', str(config_dir)]
            first = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
            binary = prefix / 'bin/ghostty-console'
            config = config_dir / 'config'
            self.assertTrue(os.access(binary, os.X_OK))
            self.assertTrue(config.exists())
            self.assertEqual(stat.S_IMODE(config_dir.stat().st_mode), 0o700)
            sentinel = '# user changes\ntimeout = 777\nfont-size = 16\n'
            config.write_text(sentinel)
            second = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
            self.assertEqual(config.read_text(), sentinel)
            result = subprocess.run([str(binary), '--config-file', str(config), '+show-config'], cwd=root, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('timeout = 777', result.stdout)
            self.assertIn('font-size = 16', result.stdout)

    def test_help_does_not_build_or_install(self):
        result = subprocess.run(['fish', '--no-config', str(ROOT / 'install.fish'), '--help'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('--system', result.stdout)

if __name__ == '__main__':
    unittest.main()
