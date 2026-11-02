"""Finite PE/Mach-O inspection; metadata is evidence, not an OS support claim.

Layouts: Microsoft's PE format specification and Apple's mach-o/loader.h:
https://learn.microsoft.com/en-us/windows/win32/debug/pe-format
https://github.com/apple-oss-distributions/xnu/blob/main/EXTERNAL_HEADERS/mach-o/loader.h
"""


def integer(data, offset, size=4):
    if offset < 0 or offset + size > len(data):
        raise ValueError("truncated executable structure")
    return int.from_bytes(data[offset:offset + size], "little")


def cstring(data, offset, end=None):
    end = len(data) if end is None else end
    if not 0 <= offset < end <= len(data):
        raise ValueError("invalid executable string offset")
    stop = data.find(b"\0", offset, end)
    if stop < 0:
        raise ValueError("unterminated executable string")
    value = data[offset:stop].decode("utf-8")
    if not value or any(ord(c) < 32 for c in value):
        raise ValueError("invalid executable string")
    return value


def inspect_pe(data):
    if data[:2] != b"MZ":
        raise ValueError("payload must be a PE32+ x86_64 executable")
    pe = integer(data, 60)
    if data[pe:pe + 4] != b"PE\0\0" or integer(data, pe + 4, 2) != 0x8664:
        raise ValueError("payload must be a PE32+ x86_64 executable")
    characteristics = integer(data, pe + 22, 2)
    optional_size = integer(data, pe + 20, 2)
    optional = pe + 24
    if (not characteristics & 2 or characteristics & 0x2000
            or optional_size < 240 or integer(data, optional, 2) != 0x20b
            or integer(data, optional + 68, 2) != 3):
        raise ValueError("expected PE32+ console executable, not DLL")
    sections = []
    for index in range(integer(data, pe + 6, 2)):
        base = optional + optional_size + index * 40
        size, rva, raw_size, raw = (integer(data, base + at) for at in (8, 12, 16, 20))
        if raw + raw_size > len(data):
            raise ValueError("truncated PE section")
        sections.append((rva, raw_size, raw))
    def locate(rva, length=1):
        for start, size, raw in sections:
            if start <= rva and rva + length <= start + size:
                return raw + rva - start, raw + size
        raise ValueError("PE import RVA is outside file-backed sections")
    directory_count = integer(data, optional + 108)
    if directory_count < 14 or 112 + directory_count * 8 > optional_size:
        raise ValueError("missing PE import directories")
    imports = []
    delayed = []
    for slot, width, name_field, result in ((1, 20, 12, imports), (13, 32, 4, delayed)):
        rva = integer(data, optional + 112 + slot * 8)
        size = integer(data, optional + 116 + slot * 8)
        if not rva and not size:
            continue
        offset, _ = locate(rva, size)
        end = offset + size
        terminated = False
        while offset + width <= end:
            entry = data[offset:offset + width]
            if not any(entry):
                terminated = True
                break
            if slot == 13 and integer(data, offset) != 1:
                raise ValueError("unsupported VA-based PE delay import")
            name_offset, name_end = locate(integer(data, offset + name_field))
            name = cstring(data, name_offset, name_end)
            if '/' in name or '\\' in name or not name.lower().endswith('.dll'):
                raise ValueError("invalid PE imported DLL name")
            result.append(name)
            offset += width
        if not terminated:
            raise ValueError("unterminated PE import directory")
    if not imports:
        raise ValueError("PE candidate has no inspected load-time imports")
    runtime = [name for name in imports + delayed if name.lower().startswith(
        ("vcruntime", "msvcp", "ucrtbase", "api-ms-win-crt-", "msvcr"))]
    return {"format": "PE32+", "arch": "x86_64", "imports": imports,
            "delay_imports": delayed, "msvc_runtime_imports": runtime,
            "subsystem_version": f"{integer(data, optional + 48, 2)}.{integer(data, optional + 50, 2)}",
            "scope": "Direct and delay DLL imports; transitive runtime resolution requires native qualification"}


def packed_version(value):
    return f"{value >> 16}.{(value >> 8) & 255}.{value & 255}"


def inspect_macho(data, arch):
    cpu = {"aarch64": 0x100000c, "x86_64": 0x1000007}[arch]
    if (data[:4] != b"\xcf\xfa\xed\xfe" or integer(data, 4) != cpu
            or integer(data, 12) != 2):
        raise ValueError(f"payload must be a thin little-endian Mach-O {arch} executable")
    count, size = integer(data, 16), integer(data, 20)
    end, offset = 32 + size, 32
    if end > len(data):
        raise ValueError("truncated Mach-O load commands")
    libraries, paths, deployment, loaders = [], [], [], []
    for _ in range(count):
        command, width = integer(data, offset), integer(data, offset + 4)
        if offset + 8 > end or width < 8 or width % 8 or offset + width > end:
            raise ValueError("invalid Mach-O load command size")
        if command in (0xc, 0x80000018, 0x8000001f, 0x80000023, 0x20):
            if width < 24:
                raise ValueError("truncated Mach-O dylib command")
            string_offset = integer(data, offset + 8)
            if string_offset < 24:
                raise ValueError("invalid Mach-O dylib name")
            libraries.append(cstring(data, offset + string_offset, offset + width))
        elif command == 0xe:
            string_offset = integer(data, offset + 8)
            if string_offset < 12:
                raise ValueError("invalid Mach-O loader name")
            loaders.append(cstring(data, offset + string_offset, offset + width))
        elif command == 0x8000001c:
            string_offset = integer(data, offset + 8)
            if string_offset < 12:
                raise ValueError("invalid Mach-O rpath")
            paths.append(cstring(data, offset + string_offset, offset + width))
        elif command == 0x32:
            if width < 24 or integer(data, offset + 8) != 1:
                raise ValueError("expected macOS build-version command")
            deployment.append({"minimum": packed_version(integer(data, offset + 12)),
                               "sdk": packed_version(integer(data, offset + 16))})
        elif command == 0x24:
            if width != 16:
                raise ValueError("invalid macOS minimum-version command")
            deployment.append({"minimum": packed_version(integer(data, offset + 8)),
                               "sdk": packed_version(integer(data, offset + 12))})
        offset += width
    if offset != end or len(deployment) != 1 or not libraries:
        raise ValueError("missing or inconsistent Mach-O dependency/deployment evidence")
    if loaders != ["/usr/lib/dyld"] or paths or any(not name.startswith(("/usr/lib/", "/System/Library/")) for name in libraries):
        raise ValueError("Mach-O candidate requires non-system libraries or runtime search paths")
    return {"format": "Mach-O64", "arch": arch, "libraries": libraries,
            "rpaths": paths, "loaders": loaders, "deployment": deployment[0],
            "scope": "Load commands and SDK metadata; oldest-system execution requires native qualification"}
