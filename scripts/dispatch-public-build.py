#!/usr/bin/env python3
"""从干净的私有源码 checkout 向 public runner 提供一次性 AES-GCM 加密输入。"""
import argparse
import hashlib
import json
import pathlib
import secrets
import subprocess
import tempfile
import time
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

REPO = 'gongfpp/config-reviewer-builds'
ROOT = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--workflow', default='windows.yml', choices=['windows.yml', 'source-kit.yml', 'development-kit.yml'])
parser.add_argument('--tools-run', help='Verified public tool-preparation workflow run ID')
parser.add_argument('--dependencies-run', help='Windows dependency preparation workflow run ID')
parser.add_argument('--clean-fixture', action='store_true', help='Remove existing compiler and SDK caches on the disposable acceptance runner')
options = parser.parse_args()

def run(args, *, input=None, check=True):
    for attempt in range(4):
        result = subprocess.run(args, cwd=ROOT, input=input, text=True, capture_output=True)
        if not result.returncode or not check:
            return result
        if attempt < 3:
            time.sleep(2)
    raise RuntimeError(f'GitHub operation failed: {args[1]} {args[2]}')

if run(['git', 'status', '--porcelain']).stdout.strip():
    raise SystemExit('请先提交改动；构建只接受可追溯的干净 Git 快照。')
isolated = True  # Each supported workflow accepts independent source secrets.
sha = run(['git', 'rev-parse', '--short', 'HEAD']).stdout.strip()
version = json.loads((ROOT / 'package.json').read_text())['version']
tag = f'source-{version}-{sha}'
secret_suffix = secrets.token_hex(6).upper() if isolated else None
key_secret = f'SOURCE_KEY_DEV_{secret_suffix}' if isolated else 'SOURCE_ARCHIVE_KEY'
hash_secret = f'SOURCE_HASH_DEV_{secret_suffix}' if isolated else 'SOURCE_ARCHIVE_SHA256'
if isolated:
    tag += f'-dev-{secret_suffix.lower()}'
archive = subprocess.check_output(['git', 'archive', '--format=tar', 'HEAD'], cwd=ROOT)
key, nonce = secrets.token_bytes(32), secrets.token_bytes(12)
with tempfile.TemporaryDirectory(prefix='reviewer-encrypted-build-') as temp:
    asset = pathlib.Path(temp) / 'source.enc'
    asset.write_bytes(nonce + AESGCM(key).encrypt(nonce, archive, b'config-reviewer-source-v1'))
    run(['gh', 'secret', 'set', key_secret, '--repo', REPO], input=key.hex())
    run(['gh', 'secret', 'set', hash_secret, '--repo', REPO], input=hashlib.sha256(archive).hexdigest())
    if run(['gh', 'release', 'view', tag, '--repo', REPO], check=False).returncode:
        notes = pathlib.Path(temp) / 'notes.md'
        notes.write_text('临时 AES-GCM 加密构建输入，不包含明文源码。成功构建后清除输入及对应 Secret。')
        run(['gh', 'release', 'create', tag, '--repo', REPO, '--target', 'main', '--title', f'临时加密构建输入 {sha}', '--notes-file', str(notes)])
    run(['gh', 'release', 'upload', tag, str(asset), '--repo', REPO, '--clobber'])
arguments = ['gh', 'workflow', 'run', options.workflow, '--repo', REPO, '-f', f'source_tag={tag}']
if isolated:
    arguments += ['-f', f'source_key_secret={key_secret}', '-f', f'source_hash_secret={hash_secret}']
if options.tools_run:
    arguments += ['-f', f'tools_run={options.tools_run}']
if options.dependencies_run:
    arguments += ['-f', f'dependencies_run={options.dependencies_run}']
if options.clean_fixture:
    arguments += ['-f', 'clean_fixture=true']
run(arguments)
descriptor = ROOT/'artifacts/build-dispatch'/f'{tag}.json'
descriptor.parent.mkdir(parents=True,exist_ok=True)
descriptor.write_text(json.dumps({'source_tag':tag,'source_commit':sha,'key_secret':key_secret,'hash_secret':hash_secret,'workflow':options.workflow},indent=2))
print(f'已启动 public Windows 构建：源码 {sha}，输入 {tag}')
print(f'构建完成后清理 Secret 名称：{key_secret}、{hash_secret}')
