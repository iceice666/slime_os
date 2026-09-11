"""Boot the rollback plane image directly for a fixed wall-clock bound, keeping all serial output."""
import os, re, subprocess, sys, tempfile, time
from pathlib import Path

sys.path.insert(0, "/space/slime_os/scripts/lib")
from harness import ROOT, load_script  # noqa: E402

R = load_script("sel4_rollback_plane", "check/check-sel4-rollback-plane.py")
os.chdir(ROOT)
out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
runs, seconds = int(sys.argv[2]), float(sys.argv[3])
boot_line = Path(sys.argv[4]).read_text()
m = re.search(r"^\[boot\] (.*)$", boot_line, re.MULTILINE)
template = m.group(1)
for i in range(1, runs + 1):
    with tempfile.TemporaryDirectory() as d:
        disk = Path(d) / "rollback-plane.img"
        R.build_fixture(disk)
        command = re.sub(r"file=[^ ]*rollback-plane\.img", f"file={disk}", template).split(" ")
        try:
            proc = subprocess.run(command, capture_output=True, text=True, timeout=seconds)
            output = proc.stdout + proc.stderr
        except subprocess.TimeoutExpired as expired:
            output = (expired.stdout or b"").decode(errors="replace") + (expired.stderr or b"").decode(errors="replace")
        (out / f"raw-{i}.log").write_text(output)
        fatal = [line for line in output.splitlines() if "FATAL" in line]
        exited = any("peer complete, exiting" in line for line in output.splitlines())
        print(f"run {i} lines={len(output.splitlines())} fatal={fatal[:1]} driver_exit_marker={exited}", flush=True)
