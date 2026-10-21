#!/usr/bin/env python3
# sim.py's answer as a page: report.py sim.json pages.json edges.json > replay.md
# A check skipped whose drvPath moved is allowed only when the pages and
# docs alone moved it (sim-pages.py: the base with only the head's page and
# docs files evaluates to the head's drvPath); anything else is a false
# negative.
import json
import sys


def main():
    sim = json.load(open(sys.argv[1]))
    roots = json.load(open(sys.argv[2]))
    edges = json.load(open(sys.argv[3])) if len(sys.argv) > 3 else []
    rows = sim["rows"]
    fns, pages = [], []
    for x in sim["false_negatives"] + sim["pages_only_skips"]:
        key = next(
            (
                k
                for k in roots
                if k.endswith(":" + x["check"]) and k.split("..")[1].startswith(x["pr"])
            ),
            None,
        )
        r = roots.get(key)
        x["roots"] = r
        ok = (
            r.get("pages_only")
            if isinstance(r, dict)
            else (r and set(r) <= {"dd-pages"})
        )
        (pages if ok else fns).append(x)
    p = print
    p("# Test selection, replayed against main\n")
    p(
        f"{len(rows)} merged pull requests ({rows[0]['pr']} to {rows[-1]['pr']}), each base..head. Ground truth: every"
    )
    p(
        "check's drvPath at both ends (`nix eval` of checks.x86_64-linux). covers.json as traced at the oldest commit,"
    )
    p(
        "never again, with what each commit's .nix files name, as CI has it. A skipped check whose drvPath moved is a"
    )
    p("false negative unless the pages and docs alone moved it (sim-pages.py).\n")
    p("## False negatives\n")
    if fns:
        p("| PR | check | job | first changes |\n|---|---|---|---|")
        for x in fns:
            p(
                f"| {x['pr']} {x['subject'][:50]} | {x['check']} | {x['job']} | {x['roots']} |"
            )
    else:
        p("None.\n")
    p("\n## Skipped although the drvPath moved: pages and docs only\n")
    if pages:
        p("| PR | check | job |\n|---|---|---|")
        for x in pages:
            p(f"| {x['pr']} {x['subject'][:50]} | {x['check']} | {x['job']} |")
    else:
        p("None.\n")
    old = sum(r["old_min"] for r in rows)
    new = sum(r["new_min"] for r in rows)
    ov = sum(r["old_vm"] for r in rows)
    nv = sum(r["new_vm"] for r in rows)
    mv = sum(r["moved_vm"] for r in rows)
    p("\n## Savings\n")
    p(
        f"- VM test jobs: {ov} before, {nv} now; {mv} of them had a moved drvPath (nothing can skip those)."
    )
    p(
        f"- Job-minutes, by the timing study's medians: {old:.0f} before, {new:.0f} now ({100 * (1 - new / old):.0f}% fewer)."
    )
    om = [(r["pr"], r["subject"], r["old_missed"]) for r in rows if r.get("old_missed")]
    p(
        f"- The old path lists skipped {sum(len(m) for *_, m in om)} checks whose drvPath moved, in {len(om)} pull requests:"
    )
    for pr, subj, m in om:
        p(f"  - {pr} {subj[:50]}: {' '.join(m)}")
    docs = [r for r in rows if r["class"] == "docs only"]
    p(f"- Docs-only pull requests: {len(docs)}, each down to style and lint.")
    gaps = sim["gaps"]
    if any(r["traced"] for r in rows):
        p(
            f"- Pull requests where a traced check read past covers.json: {len({g['pr'] for g in gaps})}."
        )
    p("")
    p(
        "| PR | change | files | chosen by | VM before | VM now | VM moved | mail now/moved | min before | min now |"
    )
    p("|---|---|---|---|---|---|---|---|---|---|")
    for r in rows:
        p(
            f"| {r['pr']} | {r['subject'][:48]} | {r['files']} | {r['class'][:40]} | {r['old_vm']} | {r['new_vm']} | {r['moved_vm']} "
            f"| {r['new_mail']}/{r['moved_mail']} | {r['old_min']} | {r['new_min']} |"
        )
    if gaps:
        p("\n## Reads past covers.json (traced again)\n")
        p("| PR | check | first missing paths |\n|---|---|---|")
        for g in gaps:
            p(
                f"| {g['pr']} | {g['check']} | {', '.join(g['missing'][:4])}{' ...' if g.get('n', 0) > 4 else ''} |"
            )
    if edges:
        p("\n## Made-up changes (edges.py)\n")
        p("| case | change | chose | ok |\n|---|---|---|---|")
        for e in edges:
            tests = (
                (": " + " ".join(e["tests"]))
                if e["tests"] and len(e["tests"]) <= 12
                else ""
            )
            p(
                f"| {e['case']} | {e['changes']} | {e['chose']}{tests} | {'yes' if e['ok'] else 'NO'} |"
            )
    return 1 if fns else 0


if __name__ == "__main__":
    sys.exit(main())
