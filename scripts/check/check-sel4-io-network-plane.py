#!/usr/bin/env python3
from __future__ import annotations
import argparse
import json
import re
import shutil
import subprocess
import sys
import threading
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"lib"))
from harness import load_qemu_profile,profile_integer,profile_text,sha256_file
from sel4_gate_markers import match_marker_contract
ROOT=Path(__file__).resolve().parents[2]; IMAGE=ROOT/"build/slime-sel4-io-network.elf"; ID=ROOT/"build/slime-sel4-io-network.identity.json"; BUILD=ROOT/"scripts/build/build-sel4.py"; FIXTURE=ROOT/"contracts/generation-manifest/v1/compositions/sel4-io-network.zti"; PINS=ROOT/"sel4/pins.toml"; TIMEOUT=240
CHAINS=(
("admission",(r"SLIME_ROOT generation admitted number=53 executables=5 instances=5 grants=3 ",r"\[network-service\] authority destinations=3 rights=connect,send,recv,listen",r"\[io-link-loopback\] LinkDevice tx-queue=4 rx-queue=4 link=up")),
("granted path",(r"\[io-network-probe\] exact tcp destination connected rights=connect,send,recv",r"\[io-network-probe\] deterministic length-prefixed transfer bytes=12 echoed=12",r"\[io-network-probe\] simultaneous denied endpoint packets=0",r"\[io-network-probe\] exact dns resolved name=echo.test address=10.0.0.2",r"\[io-network-probe\] exact udp endpoint connected rights=connect,send,recv")),
("denials",tuple(r"\[io-network-intruder\] denied "+name+r" packets=0" for name in ("alternate address","alternate port","alternate dns name","wrong transport","missing CONNECT","missing SEND","missing RECV","missing LISTEN","raw-packet attempt","resolver-wide lookup","listen without LISTEN"))+(r"\[io-network-intruder\] every denial structured packets=0",)),
("reset restart",(r"\[io-network-probe\] link reset settled=2 queues=2 buffers=2 leases=2 outstanding=0",r"\[io-network-probe\] link reset fresh epoch=2 stale epoch=1 refused reconnects=1",r"\[io-network-probe\] service restart settled=1 queues=2 buffers=2 leases=1 outstanding=0",r"\[io-network-probe\] service restart fresh epoch=3 stale completion refused")),
("authority close",(r"\[io-network-probe\] no ambient socket nic raw-packet or resolver-wide authority",r"\[io-network-probe\] io network plane complete",r"SLIME_GRAPH HEALTHY generation=53 required=5 live=0 completed=5 failed=0")),)
FAILURE_MARKERS=(r"SLIME_ROOT FATAL",r"SLIME_GRAPH FAIL",r"\[network-service\] fail: ",r"\[io-network-probe\] fail: ",r"\[io-network-intruder\] fail: ",r"Caught cap fault",r"Caught vm fault",r"panicked at ")
def fail(message):raise SystemExit(f"seL4 I/O network plane check: {message}")
def main():
 p=argparse.ArgumentParser();p.add_argument("--no-build",action="store_true");a=p.parse_args();text=FIXTURE.read_text()
 for pattern in (r'generation\s*=\s*53;',r'networkDestinations\s*=\s*\[',r'name\s*=\s*"network-service"',r'name\s*=\s*"io-network-probe"',r'name\s*=\s*"io-network-intruder"',r'name\s*=\s*"io-link-loopback"'):
  if re.search(pattern,text) is None:fail(f"fixture missing {pattern}")
 if not a.no_build:
  if subprocess.run([sys.executable,str(BUILD),"--io-network-plane"],cwd=ROOT).returncode:fail("image build failed")
 identity=json.loads(ID.read_text())
 if identity.get("variant")!="io-network" or identity.get("image",{}).get("sha256")!=sha256_file(IMAGE,fail):fail("identity mismatch")
 qemu=shutil.which("qemu-system-aarch64");profile=load_qemu_profile(fail,PINS);cmd=[qemu,"-machine",profile_text(profile,"machine",fail),"-cpu",profile_text(profile,"cpu",fail),"-smp",str(profile_integer(profile,"cpus",fail)),"-m",f"size={profile_integer(profile,'memory_mib',fail)}M","-nographic","-serial","mon:stdio","-kernel",str(IMAGE)];proc=subprocess.Popen(cmd,cwd=ROOT,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True);timer=threading.Timer(TIMEOUT,proc.kill);timer.start();lines=[]
 try:
  for line in proc.stdout:
   lines.append(line.rstrip())
   if re.search(CHAINS[-1][1][-1]+"|"+"|".join(FAILURE_MARKERS),line):break
 finally:
  timer.cancel();proc.terminate()
 transcript="\n".join(lines);match_marker_contract(transcript,CHAINS,FAILURE_MARKERS,fail);print("seL4 I/O network plane check: exact destinations, denials, reset, restart, and backend independence proved")
if __name__=="__main__":main()
