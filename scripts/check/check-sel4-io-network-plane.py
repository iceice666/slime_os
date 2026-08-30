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
("admission",(r"SLIME_ROOT generation admitted number=53 executables=5 instances=5 grants=3 ",r"\[network-service\] authority destinations=5 rights=connect,send,recv",r"\[network-service\] declared socket_limit=7 listener_limit=0 dns_record_limit=2")),
("loopback honesty",(r"\[io-link-loopback\] declared endpoint bindings=1 protocol operations=0",)),
("granted path",(r"\[io-network-probe\] tcp capabilities=1 rights=connect,send,recv",r"\[io-network-probe\] successful capability operations=2",r"\[io-network-probe\] exact destination refusals=1",r"\[io-network-probe\] dns records=1 budget_refusals=1",r"\[io-network-probe\] socket charges=2 budget_refusals=1",r"\[io-network-probe\] closed capabilities=4 shutdown=1")),
("denials",(r"\[io-network-intruder\] exact authority refusals=8",r"\[io-network-intruder\] cross-holder capability refusals=4",r"\[io-network-intruder\] rights-mask refusals=2",r"\[io-network-intruder\] structured denials=14 shutdown=1")),
("service close",(r"\[network-service\] observed requests=33 packets=7 socket_refusals=1 listener_refusals=0 dns_refusals=1 cross_holder_refusals=4",r"SLIME_GRAPH HEALTHY generation=53 required=5 live=0 completed=5 failed=0")),)
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
 transcript="\n".join(lines);match_marker_contract(transcript,CHAINS,FAILURE_MARKERS,fail);print("seL4 I/O network plane check: exact authority, per-destination budgets, structured denials, and honest backend absence proved")
if __name__=="__main__":main()
