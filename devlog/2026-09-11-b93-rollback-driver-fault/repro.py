"""Drive check-sel4-rollback-plane.py's own build/boot/check functions N times, keeping every transcript."""
import os, sys, tempfile, time
from pathlib import Path

sys.path.insert(0, "/space/slime_os/scripts/lib")
from harness import ROOT, load_script  # noqa: E402

R = load_script("sel4_rollback_plane", "check/check-sel4-rollback-plane.py")
os.chdir(ROOT)
out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
runs = int(sys.argv[2])
section, qemu_binary = R.PLATFORMS["qemu-arm-virt"]
profile = R.load_qemu_profile(R.fail, R.PINS_PATH, section)
R.build_image("qemu-arm-virt")
image = R.image_path("qemu-arm-virt")
if not image.is_file():
    raise SystemExit(f"missing image {image}")
for i in range(1, runs + 1):
    with tempfile.TemporaryDirectory() as d:
        disk = Path(d) / "rollback-plane.img"
        R.build_fixture(disk)
        t0 = time.time()
        transcript = None
        status = "pass"
        try:
            transcript = R.boot(profile, disk, section=section, qemu_binary=qemu_binary, image=image)
            (out / f"transcript-{i}.log").write_text(transcript)
            R.check_transcript(transcript)
            R.check_slots_durable(disk, 40)
        except SystemExit as error:
            status = f"FAIL: {error}"
        fatal = "FATAL" in (transcript or "")
        print(f"run {i} {status} {time.time() - t0:.1f}s fatal_marker={fatal} started={time.strftime('%FT%TZ', time.gmtime(t0))}", flush=True)
