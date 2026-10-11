# DOC 回归样例

`legacy-word.html` 是本项目编写的虚构配置，无业务数据。`legacy-word.doc` 由 macOS 自带 textutil 生成，为真正的 OLE/CFB Word 二进制文件（不是改后缀）。

复现命令：

```sh
textutil -convert doc -noload -nostore -output legacy-word.doc legacy-word.html
```

覆盖中文正文、两个连续同文段落、三列表格和中英文混合标识。textutil 生成器会将未使用的 MiniFAT 扇区填成 0；样例将此空闲扇区规范化为 CFB FREESECT（0xFFFFFFFF），其余字节不变，修正后再次用 textutil 回读确认正文一致。生产解析不修补输入，也不调用 textutil；Windows 测试直接使用已提交的二进制样例。

生成后规范化命令：

```python
from pathlib import Path
import struct
p = Path("legacy-word.doc")
b = bytearray(p.read_bytes())
size = 1 << struct.unpack_from("<H", b, 30)[0]
start = (struct.unpack_from("<I", b, 60)[0] + 1) * size
assert b[start:start + size] == bytes(size)
b[start:start + size] = b"\xff" * size
p.write_bytes(b)
```
