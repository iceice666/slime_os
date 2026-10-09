"""Linux syscall and retained-memory observations for the capacity planning exam.

This monitors the actual CLI, selects the adapter's exec and descendants, and
keeps the externally specified strace transcript only in a bounded exam file.
It is an observer, not a filesystem sandbox or an atomic memory measurement.
"""

from __future__ import annotations

import ast
from collections import defaultdict
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time


class AuditError(RuntimeError):
    """The runtime observation was incomplete or violated its declared bounds."""


@dataclass(frozen=True)
class Mutation:
    pid: int
    operation: str
    path: Path | None
    attempted_bytes: int = 0
    unnamed: bool = False
    executable: tuple[str, ...] = ()
    phase: str = "setup"


@dataclass
class AuditRun:
    completed: subprocess.CompletedProcess[str]
    mutations: tuple[Mutation, ...]
    adapter_peak_rss: int
    baseline_rss: int
    rss_samples: tuple[int, ...]
    errors: tuple[str, ...]
    phase_seen: bool
    executions: dict[int, tuple[str, ...]] = field(default_factory=dict)
    collector_pids: tuple[int, ...] = ()
    recipe_pids: tuple[int, ...] = ()

    @property
    def executables(self) -> dict[int, tuple[str, ...]]:
        """Latest successful argv per PID; mutations retain their contemporaneous argv."""
        return self.executions

    @property
    def peak_memory_delta(self) -> int:
        return max(0, self.adapter_peak_rss - self.baseline_rss)

    def assert_no_spool(
        self, allowed: Mapping[Path, int] | Callable[[Path], int | None]
    ) -> None:
        """Require exact permitted paths and bounded attempted output bytes.

        The callback must not exempt whole temporary trees. Writable opens,
        even failed ones, count as attempted mutations; unnamed files are never
        allowed. Counts are cumulative, so repeated writes cannot evade a cap.
        """
        if self.errors:
            raise AuditError("; ".join(self.errors))
        limits = allowed if callable(allowed) else lambda path: allowed.get(path)
        totals: dict[Path, int] = defaultdict(int)
        for mutation in self.mutations:
            if mutation.path is None or mutation.unnamed:
                raise AuditError(f"unresolved/unnamed mutation: {mutation}")
            if mutation.operation in ("mmap", "mprotect"):
                raise AuditError(f"unmeasurable shared mapped output: {mutation}")
            limit = limits(mutation.path)
            if limit is None:
                raise AuditError(f"unpermitted mutation: {mutation}")
            totals[mutation.path] += mutation.attempted_bytes
            if totals[mutation.path] > limit:
                raise AuditError(f"attempted output exceeds {limit}: {mutation.path}")

    def require_memory(self, maximum_delta: int = 32 * 1024 * 1024) -> None:
        if self.errors:
            raise AuditError("; ".join(self.errors))
        if not self.phase_seen or not self.baseline_rss or len(self.rss_samples) < 2:
            raise AuditError("missing adapter output-phase memory observations")
        if self.peak_memory_delta > maximum_delta:
            raise AuditError(
                f"adapter retained-memory growth {self.peak_memory_delta} exceeds {maximum_delta}"
            )


# Write buffers are printed as addresses, not stream contents. File paths retain
# normal strace escaping; -yy provides resolved directory and descriptor paths.
WRITES = "write,writev,pwrite64,pwritev,pwritev2"
SYSCALLS = (
    "execve,execveat,clone,clone3,fork,vfork,open,openat,openat2,creat,close,"
    "dup,dup2,dup3,fcntl,pipe,pipe2,chdir,fchdir,mmap,mprotect,munmap,"
    "truncate,ftruncate,rename,renameat,renameat2,link,linkat,symlink,symlinkat,"
    "unlink,unlinkat,mkdir,mkdirat,rmdir,chmod,fchmod,fchmodat,chown,fchown,"
    "fchownat,lchown,utime,utimes,utimensat,fallocate,sendfile,copy_file_range,"
    "splice,memfd_create,mknod,mknodat,setxattr,lsetxattr,fsetxattr,"
    "removexattr,lremovexattr,fremovexattr,io_uring_setup,io_uring_enter,io_uring_register,"
    + WRITES
)
LINE = re.compile(r"^\s*(\d+)\s+(.*)$")
CALL = re.compile(r"^(\w+)\((.*)\)\s+=\s+(.*)$")
QUOTED = re.compile(r'"(?:[^"\\]|\\.)*"')


def _number(value: str) -> int:
    return int(value.split("<", 1)[0].strip(), 0)


def _strings(value: str) -> list[str]:
    return [ast.literal_eval(match.group()) for match in QUOTED.finditer(value)]


def _arguments(value: str) -> list[str]:
    parts, start, depth, quoted, escape = [], 0, 0, False, False
    for offset, char in enumerate(value):
        if quoted:
            if escape:
                escape = False
            elif char == "\\":
                escape = True
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
        elif char in "([{<":
            depth += 1
        elif char in ")]}>":
            depth -= 1
        elif char == "," and depth == 0:
            parts.append(value[start:offset].strip())
            start = offset + 1
    parts.append(value[start:].strip())
    return parts


def _annotation(value: str) -> str | None:
    match = re.search(r"<(.+)>", value)
    if match is None:
        return None
    name = match.group(1).removesuffix(" (deleted)")
    return re.sub(r"<(?:char|block) \d+:\d+>$", "", name)


def _regular(value: str | None) -> bool:
    return value is None or (value.startswith("/") and value != "/dev/null")


class _Trace:
    def __init__(self, cwd: Path, adapter_token: str, recipe_token: str):
        self.cwd = cwd
        self.adapter_token = adapter_token
        self.recipe_token = recipe_token
        self.adapters: set[int] = set()
        self.parents: dict[int, int] = {}
        self.executions: dict[int, tuple[str, ...]] = {}
        self.recipes: set[int] = set()
        self.alive: set[int] = set()
        self.threads: set[int] = set()
        self.phase_seen = False
        self.fds: dict[int, dict[int, str | None]] = defaultdict(dict)
        self.dirs: dict[int, Path] = {}
        self.pending: dict[int, str] = {}
        self.events: list[Mutation] = []
        self.errors: list[tuple[int, str]] = []
        self.maps: dict[int, dict[int, tuple[int, str | None, bool]]] = defaultdict(dict)

    def path(self, pid: int, value: str, directory: str | None = None) -> Path:
        names = _strings(value)
        if len(names) != 1 or '"...' in value:
            raise ValueError(f"unresolved path: {value}")
        name = Path(names[0])
        if not name.is_absolute():
            base = _annotation(directory or "")
            if base is None and directory and not directory.startswith("AT_FDCWD"):
                base = self.fds[pid].get(_number(directory))
                if base is None:
                    raise ValueError(f"unresolved directory: {directory}")
            name = Path(base) / name if base else self.dirs.get(pid, self.cwd) / name
        # Do not resolve against the final filesystem: a transient file may have
        # disappeared, and symlink metadata itself is still a mutation attempt.
        return Path(os.path.normpath(name))

    def mutation(self, pid: int, op: str, path: Path | None, size: int = 0, unnamed=False):
        self.events.append(Mutation(
            pid, op, path, size, unnamed, self.executions.get(pid, ()),
            "output" if self.output_active() else "post-output" if self.phase_seen else "setup",
        ))

    def fd_mutation(self, pid: int, op: str, descriptor: str, size: int = 0):
        fd = _number(descriptor)
        path = _annotation(descriptor) or self.fds[pid].get(fd)
        if path is None and fd in (0, 1, 2):
            # CLI adapter standard streams are inherited pipes, never artifacts.
            return
        if _regular(path):
            self.mutation(pid, op, Path(path) if path else None, size)

    def feed(self, line: str):
        match = LINE.match(line)
        if not match:
            if line.strip():
                self.errors.append((0, f"unparsable trace line: {line[:160]}"))
            return
        pid, body = int(match.group(1)), match.group(2)
        if "<unfinished ...>" in body:
            self.pending[pid] = body.split("<unfinished ...>", 1)[0]
            return
        if body.startswith("<..."):
            resumed = re.match(r"<\.\.\. \w+ resumed>(.*)", body)
            if resumed is None or pid not in self.pending:
                self.errors.append((pid, "unmatched resumed syscall"))
                return
            body = self.pending.pop(pid) + resumed.group(1)
        if body.startswith("+++"):
            self.alive.discard(pid)
            return
        if body.startswith("---"):
            return
        match = CALL.match(body)
        if not match:
            self.errors.append((pid, f"unparsable syscall: {body[:160]}"))
            return
        op, arguments, result = match.groups()
        try:
            args = _arguments(arguments)
            success = not result.startswith("-1")
            if op in ("execve", "execveat"):
                names = _strings(arguments)
                if success:
                    self.alive.add(pid)
                    argv_index = 1 if op == "execve" else 2
                    self.executions[pid] = tuple(_strings(args[argv_index]))
                if success and any(name == self.adapter_token or name.endswith('/' + self.adapter_token)
                                   for name in names if not any(c.isspace() for c in name)):
                    self.adapters.add(pid)
                if success and any(name == self.recipe_token or name.endswith('/' + self.recipe_token)
                                   for name in names if not any(c.isspace() for c in name)):
                    self.recipes.add(pid)
                    self.phase_seen = True
            elif op in ("clone", "clone3", "fork", "vfork"):
                if success and result[0].isdigit():
                    child = _number(result.split()[0])
                    self.parents[child] = pid
                    self.alive.add(child)
                    if "CLONE_THREAD" in arguments:
                        self.threads.add(child)
                    self.executions[child] = self.executions.get(pid, ())
                    self.fds[child] = (
                        self.fds[pid] if "CLONE_FILES" in arguments else self.fds[pid].copy()
                    )
                    self.dirs[child] = self.dirs.get(pid, self.cwd)
                    self.maps[child] = self.maps[pid].copy()
            elif op in ("open", "openat", "openat2", "creat"):
                path = self.path(pid, args[1] if op.startswith("openat") else args[0],
                                 args[0] if op.startswith("openat") else None)
                writable = op == "creat" or any(
                    flag in arguments for flag in ("O_WRONLY", "O_RDWR", "O_CREAT", "O_TRUNC", "O_TMPFILE")
                )
                if writable:
                    self.mutation(pid, op, path, unnamed="O_TMPFILE" in arguments)
                if success:
                    resolved = _annotation(result) or str(path)
                    self.fds[pid][_number(result)] = resolved
                    if writable and resolved.startswith('/') and Path(resolved) != path:
                        self.mutation(pid, op, Path(resolved), unnamed="O_TMPFILE" in arguments)
            elif op == "close":
                if success:
                    self.fds[pid].pop(_number(args[0]), None)
            elif op in ("dup", "dup2", "dup3", "fcntl"):
                if success and (op != "fcntl" or "F_DUPFD" in arguments):
                    self.fds[pid][_number(result)] = (
                        _annotation(args[0]) or self.fds[pid].get(_number(args[0]))
                    )
            elif op in ("pipe", "pipe2"):
                if success:
                    for fd in re.findall(r"(\d+)<pipe:\[\d+\]>", arguments):
                        self.fds[pid][int(fd)] = "pipe"
            elif op == "chdir":
                if success:
                    self.dirs[pid] = self.path(pid, args[0])
            elif op == "fchdir":
                if success:
                    directory = _annotation(args[0]) or self.fds[pid].get(_number(args[0]))
                    if directory is None:
                        raise ValueError("unresolved fchdir")
                    self.dirs[pid] = Path(directory)
            elif op in WRITES.split(","):
                # pwritev/iovec lengths cannot be recovered from raw pointer
                # arguments; refusing them is preferable to claiming byte caps.
                fd = _number(args[0])
                destination = self.fds[pid].get(fd)
                if "writev" in op and _regular(destination) and fd not in (0, 1, 2):
                    self.errors.append((pid, f"unmeasurable vectored write: {op}"))
                size = _number(args[2])
                if op == "pwrite64":
                    size += _number(args[3])
                self.fd_mutation(pid, op, args[0], size)
            elif op == "mmap":
                if "MAP_ANONYMOUS" not in arguments and "MAP_SHARED" in arguments:
                    path = _annotation(args[4]) or self.fds[pid].get(_number(args[4]))
                    length = _number(args[1])
                    if success:
                        self.maps[pid][_number(result)] = (length, path, True)
                    if "PROT_WRITE" in arguments:
                        self.fd_mutation(pid, op, args[4], length + _number(args[5]))
            elif op == "mprotect":
                if "PROT_WRITE" in arguments:
                    address, length = _number(args[0]), _number(args[1])
                    for base, (extent, path, _) in self.maps[pid].items():
                        if address < base + extent and base < address + length and _regular(path):
                            self.mutation(pid, op, Path(path) if path else None, length)
            elif op == "munmap":
                if success:
                    self.maps[pid].pop(_number(args[0]), None)
            elif op in ("ftruncate", "fallocate", "fchmod", "fchown"):
                size = _number(args[1]) if op == "ftruncate" else 0
                if op == "fallocate":
                    size = _number(args[2]) + _number(args[3])
                self.fd_mutation(pid, op, args[0], size)
            elif op == "sendfile":
                self.fd_mutation(pid, op, args[0], _number(args[3]))
            elif op in ("copy_file_range", "splice"):
                self.fd_mutation(pid, op, args[2], _number(args[4]))
            elif op == "memfd_create":
                self.mutation(pid, op, None, unnamed=True)
                if success:
                    self.fds[pid][_number(result)] = None
            elif op in ("fsetxattr", "fremovexattr"):
                self.fd_mutation(pid, op, args[0])
            elif op.startswith("io_uring"):
                self.errors.append((pid, "io_uring mutations cannot be observed by this monitor"))
            else:
                positions = {
                    "rename": ((0, None), (1, None)),
                    "renameat": ((1, 0), (3, 2)),
                    "renameat2": ((1, 0), (3, 2)),
                    "link": ((0, None), (1, None)),
                    "linkat": ((1, 0), (3, 2)),
                    "symlink": ((1, None),), "symlinkat": ((2, 1),),
                    "unlink": ((0, None),), "unlinkat": ((1, 0),),
                    "mkdir": ((0, None),), "mkdirat": ((1, 0),),
                    "rmdir": ((0, None),), "truncate": ((0, None),),
                    "chmod": ((0, None),), "fchmodat": ((1, 0),),
                    "chown": ((0, None),), "lchown": ((0, None),),
                    "fchownat": ((1, 0),), "utime": ((0, None),),
                    "utimes": ((0, None),), "utimensat": ((1, 0),),
                    "mknod": ((0, None),), "mknodat": ((1, 0),),
                    "setxattr": ((0, None),), "lsetxattr": ((0, None),),
                    "removexattr": ((0, None),), "lremovexattr": ((0, None),),
                }
                if op not in positions:
                    raise ValueError(f"unknown monitored syscall {op}")
                for index, directory in positions[op]:
                    size = _number(args[1]) if op == "truncate" else 0
                    self.mutation(pid, op, self.path(pid, args[index], args[directory]
                                  if directory is not None else None), size)
        except (ValueError, SyntaxError, IndexError, TypeError) as error:
            self.errors.append((pid, f"{op}: {error}"))

    def descendant(self, pid: int, roots: set[int]) -> bool:
        visited = set()
        while pid not in visited:
            if pid in roots:
                return True
            visited.add(pid)
            pid = self.parents.get(pid, 0)
        return False

    def selected(self, pid: int) -> bool:
        return self.descendant(pid, self.adapters)

    def collector(self, pid: int) -> bool:
        return (self.selected(pid) and pid not in self.threads
                and not self.descendant(pid, self.recipes))

    def output_active(self) -> bool:
        return any(self.descendant(pid, self.recipes) for pid in self.alive)


def _rss(pid: int) -> int:
    try:
        with open(f"/proc/{pid}/status", encoding="ascii") as status:
            for line in status:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
    except (FileNotFoundError, ProcessLookupError):
        pass
    return 0


def run(
    command: list[str], *, cwd: Path, env: dict[str, str] | None = None,
    timeout: float = 180, adapter_token: str = "scripts/check/devloop-gate.py",
    recipe_token: str = "fixture.py", trace_limit: int = 16 * 1024 * 1024,
) -> AuditRun:
    """Trace the CLI and sample aggregate collector RSS every five milliseconds.

    Native processes outside the adapter lineage are not mutation subjects.
    Collector memory includes the adapter and its non-recipe descendants, not
    threads (whose process RSS would double-count). The fixture subtree is
    excluded. Sampling begins at fixture exec and ends when that subtree exits,
    so later receipt decoding is outside this memory phase. The latest setup
    collector sample supplies the baseline. Historical adapter_peak_rss naming
    is retained for the aggregate collector metric.
    """
    cwd = Path(cwd).absolute()
    trace = _Trace(cwd, adapter_token, recipe_token)
    samples: list[int] = []
    baseline = 0
    last_rss = 0
    fatal: list[str] = []
    with tempfile.TemporaryDirectory(prefix="capacity-strace-") as temporary:
        trace_path = Path(temporary) / "trace"
        # CLI output is bounded operator evidence, not recipe output; retain it
        # in bounded exam files to avoid deadlocking the monitor's sampling loop.
        stdout_path, stderr_path = Path(temporary) / "stdout", Path(temporary) / "stderr"
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            process = subprocess.Popen(
                ["strace", "-f", "-q", "-yy", "-s", "4096", "-o", str(trace_path),
                 "-e", f"trace={SYSCALLS}", "-e", f"raw={WRITES}", *command],
                cwd=cwd, env=env, stdout=stdout, stderr=stderr, start_new_session=True,
            )
            deadline, offset, partial = time.monotonic() + timeout, 0, b""
            while True:
                if trace_path.exists():
                    size = trace_path.stat().st_size
                    if size > trace_limit:
                        fatal.append("syscall trace exceeded bounded exam capture")
                    with trace_path.open("rb") as source:
                        source.seek(offset)
                        chunk = source.read(max(0, trace_limit - offset))
                        offset += len(chunk)
                    lines = (partial + chunk).split(b"\n")
                    partial = lines.pop()
                    for line in lines:
                        trace.feed(line.decode("utf-8", "surrogateescape"))
                rss = sum(_rss(pid) for pid in trace.alive if trace.collector(pid))
                if trace.output_active():
                    if not baseline:
                        baseline = last_rss or rss
                    if rss:
                        samples.append(rss)
                elif not trace.phase_seen and rss:
                    last_rss = rss
                if stdout_path.stat().st_size + stderr_path.stat().st_size > 2 * 1024 * 1024:
                    fatal.append("CLI output exceeded bounded exam capture")
                if time.monotonic() > deadline:
                    fatal.append("audited CLI timed out")
                if fatal:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    break
                if process.poll() is not None:
                    # After strace exits all trace writes have reached this file.
                    if not trace_path.exists() or offset == trace_path.stat().st_size:
                        break
                time.sleep(0.005)
            if partial:
                fatal.append("incomplete trailing syscall trace")
            for pid in trace.pending:
                if trace.selected(pid):
                    fatal.append("incomplete adapter syscall")
        if not trace.adapters:
            fatal.append("adapter exec was not observed")
        errors = fatal + [text for pid, text in trace.errors if not pid or trace.selected(pid)]
        with stdout_path.open("rb") as stdout, stderr_path.open("rb") as stderr:
            completed = subprocess.CompletedProcess(
                command, process.returncode,
                stdout.read(2 * 1024 * 1024).decode("utf-8", "surrogateescape"),
                stderr.read(2 * 1024 * 1024).decode("utf-8", "surrogateescape"),
            )
    return AuditRun(
        completed, tuple(event for event in trace.events if trace.selected(event.pid)),
        max(samples, default=0), baseline, tuple(samples), tuple(errors), trace.phase_seen,
        {pid: argv for pid, argv in trace.executions.items() if trace.selected(pid)},
        tuple(sorted(pid for pid in trace.executions if trace.collector(pid))),
        tuple(sorted(trace.recipes)),
    )


def check_controls() -> int:
    """Exercise transient, unnamed, mapped, descendant, and outside-tree output."""
    refused = 0
    with tempfile.TemporaryDirectory(prefix="capacity-audit-controls-") as temporary:
        root = Path(temporary)
        fixture = root / "fixture.py"
        permitted = root / "bounded"
        fixture.write_text("from pathlib import Path\nPath('bounded').write_bytes(b'ok')\n")
        good = run(["python3", str(fixture)], cwd=root, adapter_token=str(fixture),
                   recipe_token=str(fixture))
        good.assert_no_spool({permitted: 2})
        if good.completed.returncode:
            raise AuditError("positive audit control did not execute")
        controls = [
            "open('hidden', 'wb').write(b'spool')",
            "open('hidden', 'ab').write(b'spool')",
            "f=tempfile.NamedTemporaryFile(); f.write(b'spool'); f.flush(); f.close()",
            "fd,p=tempfile.mkstemp(); os.write(fd,b'spool'); os.close(fd); os.unlink(p)",
            "open('hidden','wb').write(b'spool'); os.unlink('hidden')",
            "fd=os.open('/tmp',os.O_RDWR|os.O_TMPFILE,0o600); os.write(fd,b'spool')",
            "f=open('hidden','w+b'); f.truncate(4096); m=mmap.mmap(f.fileno(),4096); m[:5]=b'spool'",
            "subprocess.run(['python3','-c',\"open('hidden','wb').write(b'spool')\"],check=True)",
            "fd,p=tempfile.mkstemp(dir='/tmp'); os.write(fd,b'spool'); os.close(fd); os.unlink(p)",
            "open('hidden','wb').write(b'spool'); os.rename('hidden','renamed'); os.unlink('renamed')",
            "open('hidden','wb').write(b'spool'); os.link('hidden','linked'); os.unlink('linked'); os.unlink('hidden')",
            "f=open('bounded','wb'); f.truncate(1048576); f.close()",
            "f=open('bounded','w+b'); f.truncate(1); m=mmap.mmap(f.fileno(),1); m[:1]=b'x'",
            "try:\\n open('/definitely-missing-capacity-parent/spool','wb')\\nexcept FileNotFoundError: pass".replace('\\n','\n'),
            "open('bounded','wb').write(b'too large')",
            "fd=os.memfd_create('spool'); os.write(fd,b'spool'); os.close(fd)",
        ]
        for source in controls:
            fixture.write_text("import os,tempfile,mmap,subprocess\n" + source + "\n")
            result = run(["python3", str(fixture)], cwd=root, adapter_token=str(fixture),
                         recipe_token=str(fixture))
            if result.completed.returncode or result.errors:
                raise AuditError(f"audit control failed to execute completely: {result.errors}")
            try:
                result.assert_no_spool({permitted: 2})
            except AuditError:
                refused += 1
            else:
                raise AuditError(f"mutation control escaped runtime monitor: {source}")
        parser = _Trace(root, "adapter.py", "fixture.py")
        parser.adapters.add(101)
        parser.feed('101 openat(AT_FDCWD, "unknown", O_WRONLY) = ???')
        parser.feed('101 <... write resumed>) = 2')
        parser.feed('101 totally unparsable mutation')
        malformed = AuditRun(subprocess.CompletedProcess([], 0, '', ''), (), 0, 0, (),
                             tuple(error for _, error in parser.errors), False)
        try:
            malformed.assert_no_spool({})
        except AuditError:
            refused += 1
        else:
            raise AuditError("malformed syscall trace was accepted")
        print(f"capacity audit: {refused} filesystem/trace controls refused")
        for stream in ("stdout", "stderr", "mixed"):
            for retained in (False, True):
                adapter = root / "adapter.py"
                descriptor = {"stdout": "1", "stderr": "2", "mixed": "1+i%2"}[stream]
                fixture.write_text(
                    "import os,time\nfor i in range(2048):\n"
                    f" os.write({descriptor},b'x'*65536)\n time.sleep(.0005)\n"
                )
                adapter.write_text(
                    "import os,selectors,subprocess,time\n"
                    "time.sleep(.05)\n"
                    f"p=subprocess.Popen(['python3',{str(fixture)!r}],stdout=subprocess.PIPE,stderr=subprocess.PIPE)\n"
                    "chunks=[]\ns=selectors.DefaultSelector()\n"
                    "s.register(p.stdout,selectors.EVENT_READ)\n"
                    "s.register(p.stderr,selectors.EVENT_READ)\n"
                    "while s.get_map():\n"
                    " for key,_ in s.select():\n"
                    "  chunk=os.read(key.fd,65536)\n"
                    "  if not chunk:\n   s.unregister(key.fileobj)\n"
                    + ("  else: chunks.append(chunk)\n" if retained else "")
                    + "p.wait()\ntime.sleep(.03)\n"
                )
                result = run(["python3", str(adapter)], cwd=root, adapter_token=str(adapter),
                             recipe_token=str(fixture))
                result.assert_no_spool({})
                print(f"capacity memory control: stream={stream} retained={retained} "
                      f"baseline={result.baseline_rss} peak={result.adapter_peak_rss} "
                      f"growth={result.peak_memory_delta} samples={len(result.rss_samples)}")
                try:
                    result.require_memory()
                except AuditError:
                    if not retained:
                        raise
                    refused += 1
                else:
                    if retained:
                        raise AuditError(f"unbounded {stream} collector escaped retained-memory monitor")
            # The adapter itself does not collect. Its exec'd helper owns both
            # pipes, so adapter-only RSS would falsely accept this control.
            helper = root / "collector.py"
            helper.write_text(adapter.read_text())
            adapter.write_text(
                "import subprocess,time\ntime.sleep(.05)\n"
                f"subprocess.run(['python3',{str(helper)!r}],check=True)\n"
            )
            result = run(["python3", str(adapter)], cwd=root, adapter_token=str(adapter),
                         recipe_token=str(fixture))
            result.assert_no_spool({})
            if result.completed.returncode or len(result.collector_pids) < 2:
                raise AuditError("helper collector control did not execute its process tree")
            if set(result.collector_pids) & set(result.recipe_pids):
                raise AuditError("recipe process was included in collector memory")
            print(f"capacity helper memory control: stream={stream} "
                  f"collectors={result.collector_pids} recipes={result.recipe_pids} "
                  f"baseline={result.baseline_rss} peak={result.adapter_peak_rss} "
                  f"growth={result.peak_memory_delta} samples={len(result.rss_samples)}")
            try:
                result.require_memory()
            except AuditError:
                refused += 1
            else:
                raise AuditError(f"unbounded {stream} helper escaped retained-memory monitor")
        fixture.write_text(
            "import subprocess,time\nrecipe_memory=bytearray(128*1024*1024)\n"
            "subprocess.run(['python3','-c','import time; data=bytearray(64*1024*1024); time.sleep(.1)'],check=True)\n"
            "time.sleep(.05)\n"
        )
        adapter.write_text(
            "import subprocess,time\ntime.sleep(.05)\n"
            f"subprocess.run(['python3',{str(fixture)!r}],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True)\n"
            "after=bytearray(128*1024*1024)\ntime.sleep(.05)\n"
        )
        result = run(["python3", str(adapter)], cwd=root, adapter_token=str(adapter),
                     recipe_token=str(fixture))
        result.assert_no_spool({Path('/dev/null'): 0})
        result.require_memory()
        if result.completed.returncode:
            raise AuditError("post-recipe decoder memory control failed to execute")
        print("capacity audit: recipe subtree and post-recipe memory excluded; helper processes included")
    print(f"capacity audit: {refused} total controls refused; bounded streaming passed all streams")
    return refused
