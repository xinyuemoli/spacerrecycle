"""Reads a Windows minidump and says where the crash was.

Written rather than reaching for a debugger: the guest has no debugging tools,
and the fault the event log reported - 0xc0000005 inside KERNELBASE - describes
*where the exception surfaced*, not what caused it. A heap corruption is
detected by the allocator and reported at the allocation call, several frames
away from the write that broke it. The interesting information is the stack and
the module layout, and both are in the file.

No symbols are needed for the question being asked, which is narrow: did the
faulting address or any return address on the stack land inside our shell
extension? A yes there is actionable on its own. A no is also actionable, since
it rules the extension out.

The format is documented in dbghelp.h; the offsets below are the AMD64 ones.
"""

import struct
import sys
from pathlib import Path

# Stream types worth reading. The numbers are from MINIDUMP_STREAM_TYPE.
STREAM_THREAD_LIST = 3
STREAM_MODULE_LIST = 4
STREAM_EXCEPTION = 6
STREAM_SYSTEM_INFO = 7
STREAM_MEMORY_LIST = 9
STREAM_MEMORY64_LIST = 12

# AMD64 CONTEXT offsets, from winnt.h.
CTX_RSP = 0x98
CTX_RBP = 0xA0
CTX_RIP = 0xF8


class Dump:
    def __init__(self, path):
        self.path = Path(path)
        self.data = self.path.read_bytes()
        self._parse_header()
        self._parse_directory()

    # ---- primitive readers ------------------------------------------------
    def u32(self, off):
        return struct.unpack_from("<I", self.data, off)[0]

    def u64(self, off):
        return struct.unpack_from("<Q", self.data, off)[0]

    def bytes_at(self, rva, size):
        return self.data[rva:rva + size]

    def string_at(self, rva):
        """A MINIDUMP_STRING: a byte length followed by UTF-16."""
        n = self.u32(rva)
        raw = self.data[rva + 4:rva + 4 + n]
        return raw.decode("utf-16-le", errors="replace")

    # ---- header and stream directory -------------------------------------
    def _parse_header(self):
        sig, version, nstreams, dir_rva = struct.unpack_from("<IIII", self.data, 0)
        if sig != 0x504D444D:
            raise SystemExit(f"not a minidump: signature {sig:#x} (expected 'MDMP')")
        self.version = version
        self.nstreams = nstreams
        self.dir_rva = dir_rva
        self.timestamp = self.u32(20)
        self.flags = self.u64(24)

    def _parse_directory(self):
        self.streams = {}
        for i in range(self.nstreams):
            off = self.dir_rva + i * 12
            stream_type, size, rva = struct.unpack_from("<III", self.data, off)
            # The last one wins if a type repeats; none of the types this reads
            # are documented to repeat.
            self.streams[stream_type] = (size, rva)

    # ---- the streams ------------------------------------------------------
    def system_info(self):
        if STREAM_SYSTEM_INFO not in self.streams:
            return None
        _, rva = self.streams[STREAM_SYSTEM_INFO]
        arch = struct.unpack_from("<H", self.data, rva)[0]
        # PROCESSOR_ARCHITECTURE_AMD64 = 9
        names = {0: "x86", 5: "ARM", 6: "IA64", 9: "AMD64", 12: "ARM64"}
        return {"arch": names.get(arch, f"?{arch}"), "arch_id": arch}

    def modules(self):
        if STREAM_MODULE_LIST not in self.streams:
            return []
        _, rva = self.streams[STREAM_MODULE_LIST]
        count = self.u32(rva)
        out = []
        # MINIDUMP_MODULE is 108 bytes, the list starts 4 bytes in.
        for i in range(count):
            m = rva + 4 + i * 108
            base = self.u64(m)
            size = self.u32(m + 8)
            ts = self.u32(m + 16)
            name_rva = self.u32(m + 20)
            try:
                name = self.string_at(name_rva)
            except Exception:
                name = "<unreadable>"
            out.append({"base": base, "size": size, "stamp": ts, "name": name})
        out.sort(key=lambda x: x["base"])
        return out

    def exception(self):
        if STREAM_EXCEPTION not in self.streams:
            return None
        _, rva = self.streams[STREAM_EXCEPTION]
        tid = self.u32(rva)
        # MINIDUMP_EXCEPTION begins 8 bytes in.
        code = self.u32(rva + 8)
        flags = self.u32(rva + 12)
        record = self.u64(rva + 16)
        address = self.u64(rva + 24)
        nparams = self.u32(rva + 32)
        params = [self.u64(rva + 40 + 8 * i) for i in range(min(nparams, 15))]
        ctx_size = self.u32(rva + 160)
        ctx_rva = self.u32(rva + 164)
        return {
            "thread_id": tid,
            "code": code,
            "flags": flags,
            "record": record,
            "address": address,
            "params": params,
            "context": self.bytes_at(ctx_rva, ctx_size) if ctx_size else b"",
        }

    def threads(self):
        if STREAM_THREAD_LIST not in self.streams:
            return []
        _, rva = self.streams[STREAM_THREAD_LIST]
        count = self.u32(rva)
        out = []
        for i in range(count):
            t = rva + 4 + i * 48
            tid = self.u32(t)
            stack_start = self.u64(t + 24)
            stack_size = self.u32(t + 32)
            stack_rva = self.u32(t + 36)
            ctx_size = self.u32(t + 40)
            ctx_rva = self.u32(t + 44)
            out.append({
                "id": tid,
                "stack_start": stack_start,
                "stack_size": stack_size,
                "stack": self.bytes_at(stack_rva, stack_size) if stack_size else b"",
                "context": self.bytes_at(ctx_rva, ctx_size) if ctx_size else b"",
            })
        return out

    def memory_ranges(self):
        """Every memory range in the dump, as (start, bytes).

        Memory64List is what a full dump uses; MemoryList is the minidump one.
        Both are read because which one appears depends on DumpType.
        """
        ranges = []
        if STREAM_MEMORY64_LIST in self.streams:
            _, rva = self.streams[STREAM_MEMORY64_LIST]
            count = self.u64(rva)
            base_rva = self.u64(rva + 8)
            off = base_rva
            for i in range(count):
                start, size = struct.unpack_from("<QQ", self.data, rva + 16 + i * 16)
                ranges.append((start, self.data[off:off + size]))
                off += size
        if STREAM_MEMORY_LIST in self.streams:
            _, rva = self.streams[STREAM_MEMORY_LIST]
            count = self.u32(rva)
            for i in range(count):
                m = rva + 4 + i * 16
                start = self.u64(m)
                size = self.u32(m + 8)
                data_rva = self.u32(m + 12)
                ranges.append((start, self.bytes_at(data_rva, size)))
        return ranges


def module_for(modules, addr):
    """The module an address belongs to, or None."""
    for m in modules:
        if m["base"] <= addr < m["base"] + max(m["size"], 1):
            return m
    return None


def describe(modules, addr):
    m = module_for(modules, addr)
    if m is None:
        return f"{addr:#018x}  <not in any module>"
    name = Path(m["name"]).name
    return f"{addr:#018x}  {name}+{addr - m['base']:#x}"


EXCEPTION_NAMES = {
    0xC0000005: "ACCESS_VIOLATION",
    0xC0000006: "IN_PAGE_ERROR",
    0xC000001D: "ILLEGAL_INSTRUCTION",
    0xC0000094: "INTEGER_DIVIDE_BY_ZERO",
    0xC0000096: "PRIV_INSTRUCTION",
    0xC00000FD: "STACK_OVERFLOW",
    0xC0000374: "HEAP_CORRUPTION",
    0xC0000409: "STACK_BUFFER_OVERRUN",
    0xC0000420: "ASSERTION_FAILURE",
    0xE06D7363: "C++ EXCEPTION (MSVC)",
    0x80000003: "BREAKPOINT",
}


def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: dump-crash.py <file.dmp>")
    d = Dump(sys.argv[1])

    print(f"file      : {d.path}  ({d.path.stat().st_size / 1e6:.1f} MB)")
    si = d.system_info()
    if si:
        print(f"arch      : {si['arch']}")

    modules = d.modules()
    print(f"modules   : {len(modules)}")

    ex = d.exception()
    if ex is None:
        print("no exception stream - this dump was not written by a crash")
    else:
        code = ex["code"]
        name = EXCEPTION_NAMES.get(code, "unknown")
        print()
        print("=== exception ===")
        print(f"  code      : {code:#010x}  {name}")
        print(f"  address   : {describe(modules, ex['address'])}")
        print(f"  thread    : {ex['thread_id']}")
        if ex["params"]:
            print(f"  parameters: " + ", ".join(f"{p:#x}" for p in ex["params"]))
        # For an access violation the first two parameters are the access type
        # and the address that was touched; that address is often the answer.
        if code == 0xC0000005 and len(ex["params"]) >= 2:
            kind = {0: "read", 1: "write", 8: "execute"}.get(ex["params"][0], str(ex["params"][0]))
            print(f"  access    : {kind} of {ex['params'][1]:#018x}")
            hit = module_for(modules, ex["params"][1])
            if hit:
                print(f"              that address is inside {Path(hit['name']).name}")

    # The faulting thread's own stack, walked by scanning rather than unwinding.
    # Without symbols an unwind is not available, but every return address on the
    # stack points into a module, and listing those in order is enough to see
    # whether our DLL was on the call path.
    if ex is not None and ex["context"]:
        ctx = ex["context"]
        rip = struct.unpack_from("<Q", ctx, CTX_RIP)[0]
        rsp = struct.unpack_from("<Q", ctx, CTX_RSP)[0]
        rbp = struct.unpack_from("<Q", ctx, CTX_RBP)[0]
        print()
        print("=== faulting thread registers ===")
        print(f"  rip = {describe(modules, rip)}")
        print(f"  rsp = {rsp:#018x}")
        print(f"  rbp = {rbp:#018x}")

        thread = next((t for t in d.threads() if t["id"] == ex["thread_id"]), None)
        if thread is None:
            print()
            print("  the faulting thread is not in the thread list")
        elif not thread["stack"]:
            print()
            print("  no stack memory in the dump")
        else:
            print()
            print("=== stack scan (module-resident 64-bit values) ===")
            stack = thread["stack"]
            start = thread["stack_start"]
            seen = 0
            for off in range(0, len(stack) - 8, 8):
                val = struct.unpack_from("<Q", stack, off)[0]
                if module_for(modules, val) is None:
                    continue
                where = describe(modules, val)
                print(f"  [{start + off:#018x}]  {where}")
                seen += 1
                if seen >= 60:
                    print("  ... (truncated)")
                    break
            if seen == 0:
                print("  nothing module-resident on the stack - likely truncated")

    # The question the whole exercise exists to answer.
    print()
    print("=== is our extension involved? ===")
    ours = [m for m in modules if "spacerecycleshell" in m["name"].lower()]
    if not ours:
        print("  the DLL was NOT loaded in the crashing process")
    else:
        for m in ours:
            print(f"  loaded: {m['name']}")
            print(f"          base {m['base']:#018x} size {m['size']:#x}")
        hits = []
        if ex is not None:
            h = module_for(modules, ex["address"])
            if h and "spacerecycleshell" in h["name"].lower():
                hits.append(("exception address", ex["address"]))
            for p in (ex["params"] or []):
                h = module_for(modules, p)
                if h and "spacerecycleshell" in h["name"].lower():
                    hits.append(("exception parameter", p))
        if ex is not None:
            thread = next((t for t in d.threads() if t["id"] == ex["thread_id"]), None)
            if thread and thread["stack"]:
                for off in range(0, len(thread["stack"]) - 8, 8):
                    val = struct.unpack_from("<Q", thread["stack"], off)[0]
                    h = module_for(modules, val)
                    if h and "spacerecycleshell" in h["name"].lower():
                        hits.append(("stack value", val))
                        break
        if hits:
            for what, addr in hits:
                print(f"  FOUND: {what} -> {describe(modules, addr)}")
        else:
            print("  NOT implicated: no address in the fault or on the stack")
            print("  points into the extension")


if __name__ == "__main__":
    main()
