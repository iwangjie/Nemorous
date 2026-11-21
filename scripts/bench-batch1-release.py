#!/usr/bin/env python3
"""
Batch 1 同机 Release 性能对比脚本（至少 10 次交错对比）：
- 对比对象：
  - 旧版 Go Release 包：build/migration-handoff/baseline/Nemorous.app/Contents/MacOS/Nemorous
  - 新版 Rust NSH Release 包：build/out/Nemorous.app/Contents/MacOS/Nemorous
- 测试仓库：build/migration-handoff/baseline/repo 的隔离副本（每次切换前后重置为完全相同的 HEAD/index/工作树字节）
- 独立隔离配置：NEMOROUS_CONFIG 与 NEMOROUS_WORKSPACE 指向 worktree 内临时目录，LSP 关闭
- 测量指标：
  1. 交错 10 轮（首启 1 次 + 热启 10 次）：进程启动至稳定/窗口创建耗时、新版精确 main_start -> commit_ready 耗时、/usr/bin/time -l 实测最大常驻内存（Peak RSS MiB）、CPU 用户/系统耗时；
  2. Agent 改盘至新版 FSEvents 自动刷新 commit_ready 延迟（10 次采样 p50/p95）；
  3. 并发子进程数检查（运行稳态下无残留泄漏子进程）；
  4. 结合 cargo test --offline --release --test perf_batch1_tests 输出大 Diff（20k 行多 hunk、12k 长行、1.92 MiB、>2 MiB 拦截、50 次切文件）。
"""

import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import time

ROOT = os.getcwd()
BASELINE_REPO = os.path.join(ROOT, "build", "migration-handoff", "baseline", "repo")
OLD_BIN = os.path.join(ROOT, "build", "migration-handoff", "baseline", "Nemorous.app", "Contents", "MacOS", "Nemorous")
NEW_BIN = os.path.join(ROOT, "build", "out", "Nemorous.app", "Contents", "MacOS", "Nemorous")
WORK = os.path.join(ROOT, "build", "tmp-ab", "bench-release")
EVIDENCE_LOG = os.path.join(ROOT, "build", "migration-evidence", "batch-1", "perf-release.log")


def reset_repo(dst: str):
    if os.path.exists(dst):
        shutil.rmtree(dst)
    shutil.copytree(BASELINE_REPO, dst)


def parse_max_rss_bytes(stderr_text: str) -> int:
    # macOS `/usr/bin/time -l` reports: "  12345678  maximum resident set size" (bytes on macOS)
    m = re.search(r"(\d+)\s+maximum resident set size", stderr_text)
    return int(m.group(1)) if m else 0


def parse_timing_events(text: str):
    events = []
    for line in text.splitlines():
        if "NEMOROUS_TIMING " in line:
            kv = {}
            for tok in line.split("NEMOROUS_TIMING ", 1)[1].split():
                if "=" in tok:
                    k, v = tok.split("=", 1)
                    kv[k] = v
            events.append(kv)
    return events


def run_once(label: str, bin_path: str, repo_dir: str, cfg_path: str, hold_sec: float = 1.6):
    env = os.environ.copy()
    env["NEMOROUS_CONFIG"] = cfg_path
    env["NEMOROUS_WORKSPACE"] = repo_dir
    env["NEMOROUS_TRACE_TIMING"] = "1"
    env["TMPDIR"] = WORK

    t0_wall = time.time() * 1000.0
    p = subprocess.Popen(
        ["/usr/bin/time", "-l", bin_path],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    time.sleep(hold_sec)
    # Terminate child cleanly (time -l waits and prints rusage of child)
    # Send SIGTERM to the process group/child via p.terminate() isn't enough for time wrapper; find exact child or send SIGTERM to time's child.
    # Instead, let's measure RSS via python's own resource.getrusage(RUSAGE_CHILDREN) OR run binary directly and sample / wait with wait4!
    p.terminate()
    try:
        _, err = p.communicate(timeout=3)
    except subprocess.TimeoutExpired:
        p.kill()
        _, err = p.communicate()
    return err, t0_wall


def run_direct_wait4(bin_path: str, repo_dir: str, cfg_path: str, hold_sec: float = 1.5, wait_for_ready: bool = False):
    """直接启动目标可执行文件，用 os.wait4 精确获取内核 rusage (ru_maxrss 字节, utime, stime)。"""
    env = os.environ.copy()
    env["NEMOROUS_CONFIG"] = cfg_path
    env["NEMOROUS_WORKSPACE"] = repo_dir
    env["NEMOROUS_TRACE_TIMING"] = "1"
    env["TMPDIR"] = WORK

    stderr_path = os.path.join(WORK, f"stderr-{os.getpid()}-{time.time_ns()}.log")
    t0_epoch_ms = time.time() * 1000.0
    with open(stderr_path, "w+") as ef:
        p = subprocess.Popen([bin_path], env=env, stdout=subprocess.DEVNULL, stderr=ef)
        ready_ms = None
        if wait_for_ready:
            deadline = time.monotonic() + 12.0
            while time.monotonic() < deadline:
                ef.flush()
                ef.seek(0)
                txt = ef.read()
                evs = parse_timing_events(txt)
                starts = [int(e["epoch_ms"]) for e in evs if e.get("event") == "main_start"]
                readies = [int(e["epoch_ms"]) for e in evs if e.get("event") == "commit_ready"]
                if readies:
                    ready_ms = readies[0] - (starts[0] if starts else int(t0_epoch_ms))
                    wall_ready_ms = readies[0] - t0_epoch_ms
                    # Keep brief steady window to match idle RSS sampling
                    time.sleep(0.35)
                    break
                time.sleep(0.02)
            else:
                wall_ready_ms = None
        else:
            time.sleep(hold_sec)
            wall_ready_ms = None

        child_pids = []
        try:
            cp = subprocess.run(["pgrep", "-P", str(p.pid)], capture_output=True, text=True, timeout=2)
            child_pids = [int(x) for x in cp.stdout.split() if x.strip().isdigit()]
        except Exception:
            pass

        p.terminate()
        try:
            pid, status, rusage = os.wait4(p.pid, 0)
        except ChildProcessError:
            rusage = None
        p.returncode = 0  # prevent Popen.__del__ warning

        ef.flush()
        ef.seek(0)
        stderr_text = ef.read()

    try:
        os.remove(stderr_path)
    except OSError:
        pass

    rss_mib = (rusage.ru_maxrss / (1024.0 * 1024.0)) if rusage else 0.0
    utime_ms = (rusage.ru_utime * 1000.0) if rusage else 0.0
    stime_ms = (rusage.ru_stime * 1000.0) if rusage else 0.0
    return {
        "rss_mib": rss_mib,
        "utime_ms": utime_ms,
        "stime_ms": stime_ms,
        "ready_ms": ready_ms,
        "wall_ready_ms": wall_ready_ms,
        "idle_child_pids": child_pids,
        "stderr": stderr_text,
    }


def git_run(repo_dir: str, args):
    subprocess.run(["git", "-C", repo_dir, *args], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def make_large_log_repo(repo_dir: str):
    if os.path.exists(repo_dir):
        shutil.rmtree(repo_dir)
    os.makedirs(repo_dir, exist_ok=True)
    git_run(repo_dir, ["init", "-b", "main"])
    git_run(repo_dir, ["config", "user.name", "Bench"])
    git_run(repo_dir, ["config", "user.email", "bench@example.com"])
    with open(os.path.join(repo_dir, "base.txt"), "w") as f:
        f.write("base\n")
    git_run(repo_dir, ["add", "base.txt"])
    git_run(repo_dir, ["commit", "-m", "initial"])
    long_msg = "commit_payload_" * 140  # ~2.1 KiB per commit -> 45 commits > 95 KiB git log
    for i in range(45):
        git_run(repo_dir, ["commit", "--allow-empty", "-m", f"{i:02d} {long_msg}"])
    with open(os.path.join(repo_dir, "base.txt"), "w") as f:
        f.write("base modified\n")


def make_2500_files_repo(repo_dir: str):
    if os.path.exists(repo_dir):
        shutil.rmtree(repo_dir)
    bulk = os.path.join(repo_dir, "bulk")
    os.makedirs(bulk, exist_ok=True)
    git_run(repo_dir, ["init", "-b", "main"])
    git_run(repo_dir, ["config", "user.name", "Bench"])
    git_run(repo_dir, ["config", "user.email", "bench@example.com"])
    for i in range(2500):
        with open(os.path.join(bulk, f"item_{i:04d}_with_longer_filename_padding.txt"), "w") as f:
            f.write("ok\n")
    git_run(repo_dir, ["add", "."])
    git_run(repo_dir, ["commit", "-m", "add 2500 tracked files"])
    with open(os.path.join(bulk, "item_0000_with_longer_filename_padding.txt"), "w") as f:
        f.write("modified\n")


def measure_agent_watch_latency(bin_path: str, repo_dir: str, cfg_path: str, iterations: int = 10):
    """测量新版 Release 进程运行中，外部 Agent 写盘到 commit_ready 刷新落盘的端到端延迟。"""
    env = os.environ.copy()
    env["NEMOROUS_CONFIG"] = cfg_path
    env["NEMOROUS_WORKSPACE"] = repo_dir
    env["NEMOROUS_TRACE_TIMING"] = "1"
    env["TMPDIR"] = WORK

    stderr_path = os.path.join(WORK, "watch-bench.log")
    latencies = []
    with open(stderr_path, "w+") as ef:
        p = subprocess.Popen([bin_path], env=env, stdout=subprocess.DEVNULL, stderr=ef)
        # Wait initial commit_ready
        deadline = time.monotonic() + 6.0
        seen_count = 0
        while time.monotonic() < deadline:
            ef.flush()
            ef.seek(0)
            evs = [e for e in parse_timing_events(ef.read()) if e.get("event") == "commit_ready"]
            if evs:
                seen_count = len(evs)
                break
            time.sleep(0.03)
        time.sleep(0.3)

        for i in range(iterations):
            target = os.path.join(repo_dir, f"agent-bench-{i}.txt")
            t_write_ms = time.time() * 1000.0
            with open(target, "w") as tf:
                tf.write(f"agent bench payload {i}\n")
            d2 = time.monotonic() + 5.0
            while time.monotonic() < d2:
                ef.flush()
                ef.seek(0)
                evs = [e for e in parse_timing_events(ef.read()) if e.get("event") == "commit_ready"]
                if len(evs) > seen_count:
                    lat = int(evs[-1]["epoch_ms"]) - t_write_ms
                    latencies.append(max(0.0, lat))
                    seen_count = len(evs)
                    break
                time.sleep(0.015)
            time.sleep(0.25)

        p.terminate()
        try:
            _, _, rusage = os.wait4(p.pid, 0)
        except ChildProcessError:
            rusage = None
        p.returncode = 0

    steady_rss_mib = (rusage.ru_maxrss / (1024.0 * 1024.0)) if rusage else 0.0
    return latencies, steady_rss_mib


def summarize(vals):
    if not vals:
        return "n/a"
    s = sorted(vals)
    p50 = statistics.median(s)
    p95_idx = min(len(s) - 1, int(round(0.95 * (len(s) - 1))))
    return f"min={s[0]:.1f} p50={p50:.1f} p95={s[p95_idx]:.1f} max={s[-1]:.1f} mean={statistics.mean(s):.1f} (n={len(s)})"


def main():
    shutil.rmtree(WORK, ignore_errors=True)
    os.makedirs(WORK, exist_ok=True)
    repo_dir = os.path.join(WORK, "repo")
    cfg_old = os.path.join(WORK, "cfg-old.json")
    cfg_new = os.path.join(WORK, "cfg-new.json")

    reset_repo(repo_dir)
    for cfg in (cfg_old, cfg_new):
        with open(cfg, "w") as f:
            json.dump({
                "workspace_path": repo_dir,
                "theme": "light",
                "lsp_enabled": False,
                "editor_font_size": 14,
                "change_diff_preview": False,
            }, f)

    lines = []
    def log(msg):
        print(msg, flush=True)
        lines.append(msg)

    log("=== Nemorous Batch 1 Same-Machine Release Performance Benchmark ===")
    log(f"OLD_BIN={OLD_BIN} ({os.path.getsize(OLD_BIN)/(1024*1024):.2f} MiB)")
    log(f"NEW_BIN={NEW_BIN} ({os.path.getsize(NEW_BIN)/(1024*1024):.2f} MiB)")
    log(f"REPO_FIXTURE={BASELINE_REPO} (isolated copy per run)")

    # Interleaved 11 runs: run 0 = first/cold-ish, runs 1..10 = warm (10 interleaved pairs)
    old_rss, new_rss = [], []
    old_cpu, new_cpu = [], []
    new_ready_ms, new_wall_ready_ms = [], []

    for i in range(11):
        tag = "cold" if i == 0 else f"warm-{i:02d}"
        reset_repo(repo_dir)
        r_new = run_direct_wait4(NEW_BIN, repo_dir, cfg_new, wait_for_ready=True)
        reset_repo(repo_dir)
        r_old = run_direct_wait4(OLD_BIN, repo_dir, cfg_old, hold_sec=1.5, wait_for_ready=False)

        log(
            f"[{tag}] NEW: peak_rss={r_new['rss_mib']:.2f}MiB ready_ms={r_new['ready_ms']} wall_ready_ms={r_new['wall_ready_ms'] and round(r_new['wall_ready_ms'],1)} cpu={r_new['utime_ms']+r_new['stime_ms']:.1f}ms idle_children={len(r_new['idle_child_pids'])} | "
            f"OLD: peak_rss={r_old['rss_mib']:.2f}MiB cpu={r_old['utime_ms']+r_old['stime_ms']:.1f}ms idle_children={len(r_old['idle_child_pids'])}"
        )
        if i >= 1:
            new_rss.append(r_new["rss_mib"])
            old_rss.append(r_old["rss_mib"])
            new_cpu.append(r_new["utime_ms"] + r_new["stime_ms"])
            old_cpu.append(r_old["utime_ms"] + r_old["stime_ms"])
            if r_new["ready_ms"] is not None:
                new_ready_ms.append(float(r_new["ready_ms"]))
            if r_new["wall_ready_ms"] is not None:
                new_wall_ready_ms.append(float(r_new["wall_ready_ms"]))

    log("\n--- Interleaved 10-run Warm Summary ---")
    log(f"NEW Peak RSS (MiB): {summarize(new_rss)}")
    log(f"OLD Peak RSS (MiB): {summarize(old_rss)}")
    log(f"NEW CPU time (ms):  {summarize(new_cpu)}")
    log(f"OLD CPU time (ms):  {summarize(old_cpu)}")
    log(f"NEW main->commit_ready (ms): {summarize(new_ready_ms)}")
    log(f"NEW spawn->commit_ready (ms): {summarize(new_wall_ready_ms)}")

    # Agent write -> watcher -> commit_ready latency (10 iterations)
    reset_repo(repo_dir)
    watch_lats, watch_rss = measure_agent_watch_latency(NEW_BIN, repo_dir, cfg_new, iterations=10)
    log("\n--- Agent Disk Write -> Watcher Auto-Refresh Latency (NEW Release) ---")
    log(f"Watch->CommitReady (ms): {summarize(watch_lats)}")
    log(f"Steady Peak RSS after 10 agent writes (MiB): {watch_rss:.2f}")

    # Large Git stdout stress repositories (verify no >64KiB pipe deadlock on shipped .app)
    log("\n--- Large Git Stdout Stress Repos (Shipped NEW Release .app) ---")
    large_log_repo = os.path.join(WORK, "repo-large-log")
    cfg_large_log = os.path.join(WORK, "cfg-large-log.json")
    make_large_log_repo(large_log_repo)
    with open(cfg_large_log, "w") as f:
        json.dump({
            "workspace_path": large_log_repo,
            "theme": "light",
            "lsp_enabled": False,
            "editor_font_size": 14,
            "change_diff_preview": False,
        }, f)
    r_ll = run_direct_wait4(NEW_BIN, large_log_repo, cfg_large_log, wait_for_ready=True)
    log(
        f"[45-commit >95KiB git-log] ready_ms={r_ll['ready_ms']} wall_ready_ms={r_ll['wall_ready_ms'] and round(r_ll['wall_ready_ms'],1)} "
        f"peak_rss={r_ll['rss_mib']:.2f}MiB idle_children={len(r_ll['idle_child_pids'])}"
    )

    large_files_repo = os.path.join(WORK, "repo-2500-files")
    cfg_large_files = os.path.join(WORK, "cfg-large-files.json")
    make_2500_files_repo(large_files_repo)
    with open(cfg_large_files, "w") as f:
        json.dump({
            "workspace_path": large_files_repo,
            "theme": "light",
            "lsp_enabled": False,
            "editor_font_size": 14,
            "change_diff_preview": False,
        }, f)
    r_lf = run_direct_wait4(NEW_BIN, large_files_repo, cfg_large_files, wait_for_ready=True)
    log(
        f"[2500-files >130KiB ls-files] ready_ms={r_lf['ready_ms']} wall_ready_ms={r_lf['wall_ready_ms'] and round(r_lf['wall_ready_ms'],1)} "
        f"peak_rss={r_lf['rss_mib']:.2f}MiB idle_children={len(r_lf['idle_child_pids'])}"
    )

    if r_ll["ready_ms"] is None or r_ll["ready_ms"] > 5000:
        raise SystemExit(f"FAILED: 45-commit large-log repo ready_ms={r_ll['ready_ms']}")
    if r_lf["ready_ms"] is None or r_lf["ready_ms"] > 5000:
        raise SystemExit(f"FAILED: 2500-files repo ready_ms={r_lf['ready_ms']}")

    os.makedirs(os.path.dirname(EVIDENCE_LOG), exist_ok=True)
    with open(EVIDENCE_LOG, "w") as f:
        f.write("\n".join(lines) + "\n")

    shutil.rmtree(WORK, ignore_errors=True)
    log(f"\nSaved: {EVIDENCE_LOG}")


if __name__ == "__main__":
    main()
