#!/usr/bin/env python3
"""生成私有、本地交付源码包；只读取已提交 Git 文件，不复制个人数据或凭据。"""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import zipfile

root = Path(__file__).resolve().parents[1]
if subprocess.check_output(['git', 'status', '--porcelain'], cwd=root, text=True).strip():
    raise SystemExit('请先提交改动，源码包只接受可追溯的干净快照。')
sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
version = json.loads((root / 'package.json').read_text())['version']
raw = subprocess.check_output(['git', 'archive', '--format=tar', 'HEAD'], cwd=root)
output = root / 'artifacts' / 'source-kit'
output.mkdir(parents=True, exist_ok=True)
package = output / f'ConfigReviewer-{version}-source-{sha[:7]}.zip'
files = {}
with tarfile.open(fileobj=io.BytesIO(raw)) as tar:
    for entry in tar:
        if entry.isfile():
            files[entry.name] = tar.extractfile(entry).read()
info = {'version': version, 'source_commit': sha, 'target': 'Windows x64',
        'build_entry': 'scripts/build-and-run.ps1', 'installer_required_for_rebuild': False,
        'requires': ['Node.js 22 LTS', 'Rust MSVC', 'MSVC C++ and Windows SDK', 'WebView2 Runtime'],
        'offline_dependencies': 'separate Windows dependency pack; extract into this source directory',
        'code_path_checks': 'Windows application regression/UI evidence is recorded in docs/compatibility-audit.md'}
files['SOURCE-INFO.json'] = json.dumps(info, ensure_ascii=False, indent=2).encode()
files['开始修改源码.md'] = files['docs/onsite-development.md']
files['SHA256SUMS.txt'] = ('\n'.join(f'{hashlib.sha256(data).hexdigest()}  {name}' for name, data in sorted(files.items())) + '\n').encode()
with zipfile.ZipFile(package, 'w', zipfile.ZIP_DEFLATED, compresslevel=6) as z:
    for name, data in sorted(files.items()):
        z.writestr(name, data)
with zipfile.ZipFile(package) as z:
    assert z.testzip() is None
    for line in z.read('SHA256SUMS.txt').decode().splitlines():
        expected, name = line.split('  ', 1)
        assert hashlib.sha256(z.read(name)).hexdigest() == expected
    for required in ['src/App.tsx', 'src-tauri/src/gateway.rs', 'src-tauri/src/tests.rs', 'src-tauri/icons/icon.ico', 'package-lock.json', 'src-tauri/Cargo.lock', 'scripts/build-and-run.ps1']:
        assert required in z.namelist()
checksum = hashlib.sha256(package.read_bytes()).hexdigest()
(output / 'SOURCE-ZIP-SHA256.txt').write_text(f'{checksum}  {package.name}\n')
print(json.dumps({'path': str(package), 'source_commit': sha, 'bytes': package.stat().st_size, 'files': len(files), 'crc_and_hashes': 'PASS'}, ensure_ascii=False))
