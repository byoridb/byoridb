#!/usr/bin/env python3
"""
CRAP score analyzer.

Joins per-function cyclomatic complexity (from clippy cognitive_complexity)
with per-function coverage (from cargo-llvm-cov JSON) and reports CRAP scores.

CRAP(m) = comp(m)^2 * (1 - cov(m))^3 + comp(m)
"""

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path


def parse_clippy_cc(clippy_text: str) -> dict[tuple[str, int], tuple[int, str]]:
    """Return {(file, line): (cc, signature)} from clippy output."""
    pat = re.compile(
        r"cognitive complexity of \((\d+)/\d+\).*?--> (\S+):(\d+):\d+\s*\n\s*\|\s*\n(\d+)\s*\|\s*([^\n]+)",
        re.DOTALL,
    )
    out = {}
    for m in pat.finditer(clippy_text):
        cc = int(m.group(1))
        f = m.group(2)
        line = int(m.group(3))
        sig = m.group(5).strip()
        # Normalize: keep highest CC if duplicate (e.g., test+lib targets)
        key = (f, line)
        if key not in out or out[key][0] < cc:
            out[key] = (cc, sig)
    return out


def parse_llvm_cov(cov_path: Path, repo_root: Path):
    """Parse cargo-llvm-cov JSON. Yield (file_rel, demangled_name, line_start, line_count, lines_covered)."""
    data = json.loads(cov_path.read_text())
    # Format: data[0].functions[*]
    for entry in data["data"]:
        for fn in entry.get("functions", []):
            name = fn.get("name", "")
            filenames = fn.get("filenames", [])
            if not filenames:
                continue
            f = filenames[0]
            # Make path relative to repo root if possible
            try:
                f_rel = str(Path(f).resolve().relative_to(repo_root.resolve()))
            except ValueError:
                f_rel = f
            regions = fn.get("regions", [])
            if not regions:
                continue
            # Each region: [line_start, col_start, line_end, col_end, exec_count, file_id, expanded_file_id, kind]
            line_start = min(r[0] for r in regions)
            line_end = max(r[2] for r in regions)
            covered_regions = sum(1 for r in regions if r[4] > 0)
            total_regions = len(regions)
            cov = covered_regions / total_regions if total_regions else 0.0
            yield (f_rel, name, line_start, line_end, cov, total_regions)


def crap_score(comp: int, cov: float) -> float:
    return comp * comp * ((1 - cov) ** 3) + comp


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--clippy", required=True, help="Path to clippy output text")
    ap.add_argument("--cov", required=True, help="Path to cargo-llvm-cov JSON")
    ap.add_argument("--repo", default=".", help="Repo root")
    ap.add_argument("--threshold", type=float, default=5.0, help="CRAP threshold")
    ap.add_argument("--top", type=int, default=30, help="Show top N offenders")
    args = ap.parse_args()

    repo_root = Path(args.repo)
    clippy_text = Path(args.clippy).read_text()
    cc_map = parse_clippy_cc(clippy_text)
    print(f"Parsed {len(cc_map)} CC entries from clippy", file=sys.stderr)

    cov_entries = list(parse_llvm_cov(Path(args.cov), repo_root))
    print(f"Parsed {len(cov_entries)} coverage entries from llvm-cov", file=sys.stderr)

    # Index coverage by file → list of (line_start, line_end, cov, name)
    by_file = defaultdict(list)
    for f, name, ls, le, cov, regions in cov_entries:
        by_file[f].append((ls, le, cov, name, regions))

    # Join: for each clippy CC entry, find coverage entry whose line range contains it
    crap_rows = []
    matched = 0
    unmatched = 0
    for (f, line), (cc, sig) in cc_map.items():
        candidates = by_file.get(f, [])
        match = None
        for ls, le, cov, name, regions in candidates:
            if ls <= line <= le:
                match = (ls, le, cov, name, regions)
                break
        if match is None:
            unmatched += 1
            cov = 0.0
            name = "<unmatched>"
        else:
            matched += 1
            cov = match[2]
            name = match[3]
        crap = crap_score(cc, cov)
        crap_rows.append((crap, cc, cov, f, line, sig, name))

    print(f"Matched {matched}/{len(cc_map)} clippy entries to coverage; {unmatched} unmatched", file=sys.stderr)
    crap_rows.sort(reverse=True)

    # Summary
    over = [r for r in crap_rows if r[0] > args.threshold]
    print(f"\n{'='*100}")
    print(f"CRAP ≤ {args.threshold} target: {len(crap_rows) - len(over)}/{len(crap_rows)} pass; {len(over)} fail")
    print(f"{'='*100}\n")

    print(f"{'CRAP':>8}  {'CC':>4}  {'Cov':>5}  Location")
    print("-" * 100)
    for crap, cc, cov, f, line, sig, name in crap_rows[: args.top]:
        loc = f"{f}:{line}"
        print(f"{crap:>8.1f}  {cc:>4}  {cov*100:>4.1f}%  {loc}")
        if len(sig) < 80:
            print(f"           {sig[:90]}")

    # Save full results as TSV
    out_tsv = Path("/tmp/crap_results.tsv")
    with out_tsv.open("w") as f_out:
        f_out.write("crap\tcc\tcov_pct\tfile\tline\tsignature\n")
        for crap, cc, cov, f, line, sig, name in crap_rows:
            f_out.write(f"{crap:.2f}\t{cc}\t{cov*100:.1f}\t{f}\t{line}\t{sig}\n")
    print(f"\nFull results: {out_tsv} ({len(crap_rows)} rows)")


if __name__ == "__main__":
    main()
