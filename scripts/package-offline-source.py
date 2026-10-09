#!/usr/bin/env python3
"""本地合并源码和已验证 Windows 依赖；不将明文项目源码上传 public。"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--dependencies', type=Path, required=True)
options = parser.parse_args()
subprocess.run(['python3', str(root/'scripts/package-source.py')], cwd=root, check=True)
sha = subprocess.check_output(['git','rev-parse','HEAD'], cwd=root, text=True).strip()
version = json.loads((root/'package.json').read_text())['version']
folder = root/'artifacts/source-kit'
source = folder/f'ConfigReviewer-{version}-source-{sha[:7]}.zip'
output = folder/f'ConfigReviewer-{version}-windows-x64-source-offline.zip'
excluded = {'SHA256SUMS.txt','DEPENDENCY-SHA256SUMS.txt'}
with zipfile.ZipFile(source) as src, zipfile.ZipFile(options.dependencies) as deps, zipfile.ZipFile(output,'w',zipfile.ZIP_DEFLATED,compresslevel=6) as target:
    info = json.loads(deps.read('DEPENDENCY-INFO.json'))
    assert info['offline_build_verified'] and info['direct_exe_ui']['passed'] and info['no_installer_generated']
    dep_names = set(deps.namelist())
    hashes = []
    for archive in [src,deps]:
        for name in sorted(archive.namelist()):
            if name in excluded or (archive is src and name in dep_names):
                continue
            data = archive.read(name)
            if name == 'SOURCE-INFO.json':
                source_info = json.loads(data)
                source_info['validated_code_commit'] = info['source_commit']
                source_info['offline_dependencies'] = 'included under .offline; no separate extraction needed'
                source_info['toolchains_included'] = False
                data = json.dumps(source_info,ensure_ascii=False,indent=2).encode()
            target.writestr(name,data)
            hashes.append(f'{hashlib.sha256(data).hexdigest()}  {name}')
    target.writestr('SHA256SUMS.txt','\n'.join(hashes)+'\n')
with zipfile.ZipFile(output) as package:
    assert package.testzip() is None
    for line in package.read('SHA256SUMS.txt').decode().splitlines():
        expected,name=line.split('  ',1)
        with package.open(name) as stream:
            assert hashlib.file_digest(stream,'sha256').hexdigest()==expected,name
    assert 'src/App.tsx' in package.namelist() and 'scripts/build-and-run.ps1' in package.namelist()
    assert b'replace-with = "vendored-sources"' in package.read('.cargo/config.toml')
checksum = hashlib.file_digest(output.open('rb'),'sha256').hexdigest()
(folder/'OFFLINE-SOURCE-ZIP-SHA256.txt').write_text(f'{checksum}  {output.name}\n')
print(json.dumps({'path':str(output),'bytes':output.stat().st_size,'source_commit':sha,'validated_code_commit':info['source_commit'],'files':len(hashes)+1,'crc_and_hashes':'PASS','sha256':checksum},ensure_ascii=False))
