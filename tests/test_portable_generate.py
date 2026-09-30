import contextlib
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import runpy
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / 'libs' / 'portable' / 'generate.py'
SPEC = importlib.util.spec_from_file_location('portable_generate', SCRIPT)
generate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(generate)


class PortableGenerateTests(unittest.TestCase):
    def setUp(self):
        self.original_cwd = Path.cwd()
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.source = self.root / 'app'
        self.source.mkdir()
        (self.source / 'rustdesk.exe').write_bytes(b'executable\x00\xff')
        (self.source / 'nested').mkdir()
        (self.source / 'nested' / 'resource.txt').write_bytes('家人协助'.encode())

    def tearDown(self):
        os.chdir(self.original_cwd)
        self.temp.cleanup()

    def generate_table(self):
        with contextlib.redirect_stdout(io.StringIO()):
            return generate.generate_md5_table(str(self.source), 1)

    def test_compression_preserves_cwd_and_package_paths(self):
        table = self.generate_table()
        self.assertEqual(Path.cwd(), self.original_cwd)
        self.assertEqual(set(table), {
            os.path.join('.', 'rustdesk.exe'),
            os.path.join('.', 'nested', 'resource.txt'),
        })
        for path, (compressed, digest) in table.items():
            original = (self.source / path).read_bytes()
            self.assertEqual(generate.brotli.decompress(compressed), original)
            self.assertEqual(digest, hashlib.md5(original).hexdigest().encode())

    def test_compression_error_preserves_cwd(self):
        with patch.object(generate.brotli, 'compress', side_effect=RuntimeError('compression failed')):
            with self.assertRaises(RuntimeError), contextlib.redirect_stdout(io.StringIO()):
                generate.generate_md5_table(str(self.source), 1)
        self.assertEqual(Path.cwd(), self.original_cwd)

    def test_missing_source_is_rejected(self):
        with self.assertRaises(NotADirectoryError):
            generate.generate_md5_table(str(self.root / 'missing'), 1)

    def test_executable_accepts_relative_and_absolute_paths(self):
        os.chdir(self.root)
        expected = os.path.join('.', 'rustdesk.exe')
        for executable in ['rustdesk.exe', os.path.join('app', 'rustdesk.exe'),
                           str(self.source / 'rustdesk.exe')]:
            with self.subTest(executable=executable):
                self.assertEqual(generate.resolve_executable('app', executable), expected)

    def test_executable_accepts_nested_paths(self):
        self.assertEqual(
            generate.resolve_executable(str(self.source), os.path.join('nested', 'resource.txt')),
            os.path.join('.', 'nested', 'resource.txt'),
        )

    def test_executable_rejects_sibling_with_same_prefix(self):
        sibling = self.root / 'app-other'
        sibling.mkdir()
        executable = sibling / 'rustdesk.exe'
        executable.write_bytes(b'outside')
        with self.assertRaisesRegex(ValueError, 'inside the source folder'):
            generate.resolve_executable(str(self.source), str(executable))

    def test_executable_rejects_parent_traversal(self):
        (self.root / 'outside.exe').write_bytes(b'outside')
        with self.assertRaisesRegex(ValueError, 'inside the source folder'):
            generate.resolve_executable(str(self.source), os.path.join('..', 'outside.exe'))

    def test_executable_rejects_missing_files_and_directories(self):
        for executable in ['missing.exe', 'nested']:
            with self.subTest(executable=executable):
                with self.assertRaisesRegex(ValueError, 'existing file'):
                    generate.resolve_executable(str(self.source), executable)

    def test_executable_rejects_symlink_escape(self):
        outside = self.root / 'outside.exe'
        outside.write_bytes(b'outside')
        link = self.source / 'link.exe'
        try:
            link.symlink_to(outside)
        except OSError as error:
            self.skipTest(f'Symlinks unavailable: {error}')
        with self.assertRaises(ValueError):
            generate.resolve_executable(str(self.source), 'link.exe')

    def test_cli_rejects_invalid_executable_before_writing_package(self):
        output = self.root / 'output'
        output.mkdir()
        result = subprocess.run(
            [sys.executable, str(SCRIPT), '-f', str(self.source),
             '-o', str(output), '-e', 'missing.exe'],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn('existing file', result.stderr)
        self.assertEqual(list(output.iterdir()), [])

    def test_package_wire_format_round_trip(self):
        table = self.generate_table()
        exe = generate.resolve_executable(str(self.source), 'rustdesk.exe')
        with contextlib.redirect_stdout(io.StringIO()):
            generate.write_package_metadata(table, str(self.root), exe)
        stream = io.BytesIO((self.root / 'data.bin').read_bytes())
        self.assertEqual(stream.read(8), b'rustdesk')
        for expected_path, (expected_data, expected_md5) in table.items():
            path_length = int.from_bytes(stream.read(4), 'big')
            self.assertEqual(stream.read(path_length).decode(), expected_path)
            data_length = int.from_bytes(stream.read(4), 'big')
            self.assertEqual(stream.read(data_length), expected_data)
            self.assertEqual(stream.read(32), expected_md5)
        self.assertEqual(stream.read(8), b'rustdesk')
        self.assertEqual(stream.read().decode(), exe)


    def test_legacy_windows_build_packages_existing_executable(self):
        build_script = SCRIPT.parents[2] / 'build.py'
        build_spec = importlib.util.spec_from_file_location('rustdesk_build', build_script)
        build = importlib.util.module_from_spec(build_spec)
        build_spec.loader.exec_module(build)
        (self.root / 'libs' / 'virtual_display' / 'dylib').mkdir(parents=True)
        packer = self.root / 'libs' / 'portable'
        packer.mkdir()
        shutil.copy2(SCRIPT, packer / 'generate.py')
        target = self.root / 'target' / 'release'
        target.mkdir(parents=True)
        (self.root / 'Cargo.toml').write_text('version = "1.4.9"\n')

        def compile_packer(command, check):
            self.assertEqual(command, ['cargo', 'build', '--locked', '--release'])
            self.assertTrue(check)
            (target / 'rustdesk-portable-packer.exe').write_bytes(b'portable executable')

        def run_build_command(command):
            parts = shlex.split(command)
            if parts[0] == 'cargo':
                if Path.cwd() == self.root:
                    (target / 'rustdesk.exe').write_bytes(b'application executable')
            elif parts[0] == 'mv':
                shutil.move(parts[1], parts[2])
            elif parts[0] == 'cp':
                shutil.copy2(parts[-2], parts[-1])
            elif parts[:2] == ['python3', './generate.py']:
                with patch.object(sys, 'argv', parts[1:]), \
                        patch.object(subprocess, 'run', side_effect=compile_packer):
                    runpy.run_path(parts[1], run_name='__main__')
            elif parts[:2] not in [['python3', 'res/inline-sciter.py'], ['pip3', 'install']]:
                self.fail(f'Unexpected build command: {command}')

        os.chdir(self.root)
        with patch.object(build, 'windows', True), \
                patch.object(build, 'osx', False), \
                patch.object(build, 'exe_path', 'target/release/rustdesk.exe'), \
                patch.object(build, 'external_resources'), \
                patch.object(build, 'system2', side_effect=run_build_command), \
                patch.dict(os.environ, {'P': ''}), \
                patch.object(sys, 'argv', ['build.py']), \
                contextlib.redirect_stdout(io.StringIO()):
            build.main()

        self.assertEqual(Path.cwd(), self.root)
        self.assertEqual((self.root / 'rustdesk-1.4.9-win7-install.exe').read_bytes(),
                         b'portable executable')
        self.assertFalse((target / 'rustdesk-portable-packer.exe').exists())
        self.assertTrue((packer / 'data.bin').read_bytes().endswith(
            os.path.join('.', 'RustDesk.exe').encode()))


if __name__ == '__main__':
    unittest.main()
