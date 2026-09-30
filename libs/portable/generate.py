#!/usr/bin/env python3

import os
import optparse
import subprocess
from hashlib import md5
import brotli
import datetime
from pathlib import Path

# 4GB maximum
length_count = 4
# encoding
encoding = 'utf-8'

# output: {path: (compressed_data, file_md5)}


def generate_md5_table(folder: str, level) -> dict:
    res: dict = dict()
    if not os.path.isdir(folder):
        raise NotADirectoryError(folder)
    for root, dirs, files in os.walk(folder):
        dirs.sort()
        for name in sorted(files):
            full_path = os.path.join(root, name)
            print(f"Processing {full_path}...")
            with open(full_path, "rb") as f:
                content = f.read()
            content_compressed = brotli.compress(
                content, quality=level)
            md5_code = md5(content).hexdigest().encode(encoding=encoding)
            package_path = os.path.join('.', os.path.relpath(full_path, folder))
            res[package_path] = (content_compressed, md5_code)
    return res


def resolve_executable(folder: str, executable: str) -> str:
    folder_path = Path(folder).resolve()
    executable_path = Path(executable)
    if not executable_path.is_absolute():
        cwd_path = executable_path.resolve()
        try:
            cwd_path.relative_to(folder_path)
            executable_path = cwd_path
        except ValueError:
            executable_path = folder_path / executable_path
    executable_path = executable_path.resolve()
    try:
        relative_path = executable_path.relative_to(folder_path)
    except ValueError:
        raise ValueError("The executable must be inside the source folder") from None
    if not executable_path.is_file():
        raise ValueError("The executable must be an existing file")
    return os.path.join('.', str(relative_path))


def write_package_metadata(md5_table: dict, output_folder: str, exe: str):
    output_path = os.path.join(output_folder, "data.bin")
    with open(output_path, "wb") as f:
        f.write("rustdesk".encode(encoding=encoding))
        for path in md5_table.keys():
            (compressed_data, md5_code) = md5_table[path]
            data_length = len(compressed_data)
            path = path.encode(encoding=encoding)
            # path length & path
            f.write((len(path)).to_bytes(length=length_count, byteorder='big'))
            f.write(path)
            # data length & compressed data
            f.write(data_length.to_bytes(
                length=length_count, byteorder='big'))
            f.write(compressed_data)
            # md5 code
            f.write(md5_code)
        # end
        f.write("rustdesk".encode(encoding=encoding))
        # executable
        f.write(exe.encode(encoding='utf-8'))
    print(f"Metadata has been written to {output_path}")

def write_app_metadata(output_folder: str):
    output_path = os.path.join(output_folder, "app_metadata.toml")
    with open(output_path, "w") as f:
        f.write(f"timestamp = {int(datetime.datetime.now().timestamp() * 1000)}\n")
    print(f"App metadata has been written to {output_path}")

def build_portable(output_folder: str, target: str):
    current_dir = os.getcwd()
    try:
        os.chdir(output_folder)
        cmd = ["cargo", "build", "--locked", "--release"]
        if target:
            cmd.extend(["--target", target])
        subprocess.run(cmd, check=True)
    finally:
        os.chdir(current_dir)

# Linux: python3 generate.py -f ../rustdesk-portable-packer/test -o . -e ./test/main.py
# Windows: python3 .\generate.py -f ..\rustdesk\flutter\build\windows\runner\Debug\ -o . -e ..\rustdesk\flutter\build\windows\runner\Debug\rustdesk.exe


if __name__ == '__main__':
    parser = optparse.OptionParser()
    parser.add_option("-f", "--folder", dest="folder",
                      help="folder to compress")
    parser.add_option("-o", "--output", dest="output_folder",
                      help="the root of portable packer project, default is './'")
    parser.add_option("-e", "--executable", dest="executable",
                      help="specify startup file in --folder, default is rustdesk.exe")
    parser.add_option("-t", "--target", dest="target",
                      help="the target used by cargo")
    parser.add_option("-l", "--level", dest="level", type="int",
                      help="compression level, default is 11, highest", default=11)
    (options, args) = parser.parse_args()
    folder = options.folder or './rustdesk'
    output_folder = os.path.abspath(options.output_folder or './')

    try:
        exe = resolve_executable(folder, options.executable or 'rustdesk.exe')
    except ValueError as error:
        parser.error(str(error))
    print("Executable path: " + exe)
    print("Compression level: " + str(options.level))
    md5_table = generate_md5_table(folder, options.level)
    write_package_metadata(md5_table, output_folder, exe)
    write_app_metadata(output_folder)
    build_portable(output_folder, options.target)
