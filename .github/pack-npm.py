# Stage the two npm packages that carry the Rust sources of the CEF runtime and bindings.
# Usage: pack-npm.py <version> <runtime checkout> <bindings checkout> <output directory>
# The version is the NY CEF version the runtime, the bindings and the CEF build belong to.
import json, re, shutil, subprocess, sys
from pathlib import Path

version = sys.argv[1]
runtime, bindings, out = (Path(arg).resolve() for arg in sys.argv[2:5])
shutil.rmtree(out, ignore_errors=True)

def archive(repo: Path, dest: Path, paths: list[str]):
    dest.mkdir(parents=True)
    tar = subprocess.run(['git', '-C', str(repo), 'archive', 'HEAD', *paths], check=True, capture_output=True).stdout
    subprocess.run(['tar', '-x', '-C', str(dest)], input=tar, check=True)

def manifest(name: str, description: str, repository: str, files: list[str]) -> dict:
    return {
        'name': name,
        'version': version,
        'description': description,
        'license': 'Apache-2.0 OR MIT',
        'files': files,
        'repository': {'type': 'git', 'url': f'git+https://github.com/{repository}.git'},
        'publishConfig': {'access': 'public', 'registry': 'https://registry.npmjs.org'},
    }

# Bindings: the three crates in a workspace of their own, so their shared settings resolve.
package = out / 'cef-rs'
archive(bindings, package / 'rust', ['Cargo.toml', 'cef', 'sys', 'download-cef', 'LICENSE-APACHE', 'LICENSE-MIT'])
root = package / 'rust/Cargo.toml'
text = root.read_text()
text, count = re.subn(r'members = \[.*?\]', 'members = ["download-cef", "sys", "cef"]', text, count=1, flags=re.S)
assert count == 1
root.write_text(text)
for licence in ['LICENSE-APACHE', 'LICENSE-MIT']:
    shutil.copy(package / 'rust' / licence, package / licence)
(package / 'package.json').write_text(json.dumps(manifest(
    '@nyssance/cef-rs',
    'Rust bindings for NY CEF: the cef and cef-dll-sys crates generated from the build of the same version',
    'nyssance/cef-rs', ['rust', 'README.md', 'LICENSE-APACHE', 'LICENSE-MIT']), indent=2) + '\n')
(package / 'README.md').write_text(f'''# @nyssance/cef-rs

The `cef` and `cef-dll-sys` crates as maintained for [NY CEF](https://github.com/nyssance/ny-cef),
generated from NY CEF {version}. They keep the names of the crates they derive from
([tauri-apps/cef-rs](https://github.com/tauri-apps/cef-rs)), so a project takes them in place of
those with a Cargo patch:

```toml
[patch.crates-io]
cef = {{ path = "../node_modules/@nyssance/cef-rs/rust/cef" }}
cef-dll-sys = {{ path = "../node_modules/@nyssance/cef-rs/rust/sys" }}
```

Built with the `surface-embed` feature, `cef-dll-sys` downloads the NY CEF build of its own version
and verifies it by its checksum. The bindings, the runtime
([`@nyssance/tauri-runtime-cef`](https://www.npmjs.com/package/@nyssance/tauri-runtime-cef)) and the
CEF build belong together: use the same version of all three.
''')

# Runtime: the crate as it is.
package = out / 'tauri-runtime-cef'
archive(runtime, package / 'rust', ['.', ':!.github'])
for extra in ['Cargo.lock', 'Cargo.toml.orig']:
    (package / 'rust' / extra).unlink(missing_ok=True)
for licence in ['LICENSE-APACHE', 'LICENSE-MIT']:
    shutil.copy(out / 'cef-rs' / licence, package / licence)
(package / 'package.json').write_text(json.dumps(manifest(
    '@nyssance/tauri-runtime-cef',
    'The Tauri runtime for CEF as maintained for NY CEF, with embedded browser surfaces',
    'nyssance/tauri-runtime-cef', ['rust', 'README.md', 'LICENSE-APACHE', 'LICENSE-MIT']), indent=2) + '\n')
(package / 'README.md').write_text(f'''# @nyssance/tauri-runtime-cef

The `tauri-runtime-cef` crate as maintained for [NY CEF](https://github.com/nyssance/ny-cef): the
Tauri runtime for the Chromium Embedded Framework, with browser surfaces embedded in an
application's own window and the browser's requests (tabs, popups, permissions, dialogs, downloads,
context menus) handed to the application. It keeps the name of the crate it derives from
([tauri-apps/tauri](https://github.com/tauri-apps/tauri)), so a project takes it in place of that
one with a Cargo patch:

```toml
[patch.crates-io]
tauri-runtime-cef = {{ path = "../node_modules/@nyssance/tauri-runtime-cef/rust" }}
```

It needs the bindings of the same version
([`@nyssance/cef-rs`](https://www.npmjs.com/package/@nyssance/cef-rs)), patched in the same way.
This is version {version}, for NY CEF {version}.
''')
print(out)
