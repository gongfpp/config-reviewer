#!/usr/bin/env python3
"""Locally combine private source with verified public third-party tools and EXE."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--tools', type=Path, required=True)
parser.add_argument('--dependencies', type=Path, required=True)
parser.add_argument('--exe', type=Path, required=True)
parser.add_argument('--evidence', type=Path, required=True)
parser.add_argument('--validation-run', required=True)
args = parser.parse_args()
subprocess.run(['python3',str(root/'scripts/package-source.py')],cwd=root,check=True)
commit = subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
version = json.loads((root/'package.json').read_text())['version']
source = root/f'artifacts/source-kit/ConfigReviewer-{version}-source-{commit[:7]}.zip'
folder = root/'artifacts/full-dev-kit'
folder.mkdir(parents=True,exist_ok=True)
output = folder/f'ConfigReviewer-{version}-windows-x64-full-development-kit.zip'
evidence = json.loads((args.evidence/'result.json').read_text(encoding='utf-8-sig'))
validated_commit = evidence['source_commit']
changed = subprocess.check_output(['git','diff','--name-only',validated_commit,'HEAD'],cwd=root,text=True).splitlines()
allowed = {'.gitignore','README.md','scripts/package-full-development-kit.py','scripts/package-source.py'}
if any(name not in allowed and not name.startswith('docs/') for name in changed):
    raise SystemExit('Runtime or development entrypoints changed after the acceptance run; retest before packaging.')
if not evidence['passed'] or not evidence['isolation']['development_tool_outbound_blocked'] or evidence['source_modification_rebuilds'] != 2:
    raise SystemExit('Full offline environment acceptance did not pass.')
hashes = {}
with zipfile.ZipFile(source) as src, zipfile.ZipFile(args.tools) as tools, zipfile.ZipFile(args.dependencies) as deps, zipfile.ZipFile(output,'w',zipfile.ZIP_DEFLATED,compresslevel=6,strict_timestamps=False) as target:
    tool_info = json.loads(tools.read('tools/TOOLS-INFO.json').decode('utf-8-sig'))
    assert tool_info['vs_layout_verified'] and tool_info['toolchains_included']
    # Recheck every downloaded third-party file, not just the outer ZIP.
    for archive,manifest in [(tools,'TOOLS-SHA256SUMS.txt'),(deps,'DEPENDENCY-SHA256SUMS.txt')]:
        assert archive.testzip() is None
        for line in archive.read(manifest).decode('utf-8-sig').splitlines():
            expected,name=line.split('  ',1)
            with archive.open(name) as stream:
                assert hashlib.file_digest(stream,'sha256').hexdigest()==expected,name
    dep_names=set(deps.namelist())
    for archive in [src,deps,tools]:
        for name in sorted(archive.namelist()):
            if name.endswith('/') or name in {'SHA256SUMS.txt','DEPENDENCY-SHA256SUMS.txt'} or (archive is src and name in dep_names):
                continue
            assert not name.startswith(('/','\\')) and '..' not in Path(name).parts,name
            assert name not in hashes,name
            with archive.open(name) as reader, target.open(name,'w',force_zip64=True) as writer:
                digest=hashlib.sha256()
                if name=='SOURCE-INFO.json':
                    info=json.loads(reader.read())
                    info.update(toolchains_included=True,offline_dependencies='included',environment_install_entry='1-install-environment.cmd',windows10_target_tested=evidence['windows10_target_tested'],validation_run=args.validation_run,validated_source_commit=evidence.get('source_commit','see validation run'))
                    data=json.dumps(info,ensure_ascii=False,indent=2).encode()
                    digest.update(data);writer.write(data)
                else:
                    while block:=reader.read(1024*1024):
                        digest.update(block);writer.write(block)
                hashes[name]=digest.hexdigest()
    for file,name in [(args.exe,'ready-to-run/config-reviewer.exe')]+[(p,'validation/'+p.name) for p in sorted(args.evidence.iterdir()) if p.is_file()]:
        with file.open('rb') as reader,target.open(name,'w',force_zip64=True) as writer:
            digest=hashlib.sha256()
            while block:=reader.read(1024*1024):
                digest.update(block);writer.write(block)
        hashes[name]=digest.hexdigest()
    instructions=src.read('docs/offline-development-kit.md')
    target.writestr('开始使用.md',instructions)
    hashes['开始使用.md']=hashlib.sha256(instructions).hexdigest()
    target.writestr('SHA256SUMS.txt','\n'.join(f'{digest}  {name}' for name,digest in sorted(hashes.items()))+'\n')
with zipfile.ZipFile(output) as package:
    assert package.testzip() is None
    for line in package.read('SHA256SUMS.txt').decode().splitlines():
        expected,name=line.split('  ',1)
        with package.open(name) as stream:
            assert hashlib.file_digest(stream,'sha256').hexdigest()==expected,name
    for name in ['tools/node/node.exe','tools/rust/bin/rustc.exe','tools/rust/bin/cargo.exe','tools/vs-layout/vs_buildtools.exe','tools/installers/MicrosoftEdgeWebView2RuntimeInstallerX64.exe','1-install-environment.cmd','2-build-test-run.cmd','src-tauri/src/tests.rs','ready-to-run/config-reviewer.exe']:
        assert name in package.namelist(),name
with output.open('rb') as stream:
    sha=hashlib.file_digest(stream,'sha256').hexdigest()
(folder/'FULL-KIT-SHA256.txt').write_text(f'{sha}  {output.name}\n')
print(json.dumps({'path':str(output),'bytes':output.stat().st_size,'files':len(hashes)+1,'sha256':sha,'source_commit':commit,'validation_run':args.validation_run,'crc_and_hashes':'PASS'},ensure_ascii=False))
