"""Markdown and JSON report rendering."""

from __future__ import annotations

import json
from pathlib import Path


def _fmt_s(value: float | None) -> str:
    if value is None:
        return "—"
    if value >= 1.0:
        return f"{value:.3f} s"
    return f"{value * 1000:.2f} ms"


def _table(rows: list[list[str]], headers: list[str]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "|" + "|".join(["---"] * len(headers)) + "|"]
    for row in rows:
        lines.append("| " + " | ".join(str(cell) for cell in row) + " |")
    return "\n".join(lines)


def render_markdown(
    *,
    run: dict,
    identity: dict,
    gate: dict,
    fixtures: dict,
    packs_root: str | None,
) -> str:
    machine = identity.get("machine", {})
    platform = machine.get("platform", {})
    toolchains = machine.get("toolchains", {})
    binary = identity.get("rust_binary", {})
    source = identity.get("source", {})
    deps = identity.get("dependencies", {})

    lines: list[str] = []
    lines.append("# Spec Kitty Rust vs Python benchmark")
    lines.append("")
    lines.append(f"**Gate verdict: `{gate['verdict']}`**")
    lines.append("")
    for reason in gate.get("reasons", []):
        lines.append(f"- {reason}")
    lines.append("")

    lines.append("## Measurement context")
    lines.append("")
    lines.append(
        _table(
            [
                ["Host", str(machine.get("hostname"))],
                ["CPU", f"{platform.get('cpu_model')} × {platform.get('cpu_count')}"],
                ["Memory", f"{(platform.get('mem_total_bytes') or 0) / (1024**3):.1f} GiB"],
                ["OS", f"{platform.get('system')} {platform.get('release')}"],
                ["Load at end", json.dumps(run.get("load", {}).get("loadavg", {}))],
                ["Rust toolchain", (toolchains.get("rustc") or {}).get("stdout", "").splitlines()[0] if (toolchains.get("rustc") or {}).get("stdout") else "—"],
                ["Oracle Python", (toolchains.get("oracle_python") or {}).get("stdout", "").splitlines()[0] if (toolchains.get("oracle_python") or {}).get("stdout") else "—"],
                ["Repo HEAD", source.get("repo_short_head", {}).get("stdout", "—") if isinstance(source.get("repo_short_head"), dict) else str(source.get("repo_short_head"))],
                ["Rust binary sha256", (binary.get("sha256") or "—")[:32]],
                ["uv.lock sha256", (identity.get("uv_lock_sha256") or "—")[:32]],
                ["Dependency identity", deps.get("status", "—")],
                ["Packs root", packs_root or "not configured (library adapters do not need it)"],
            ],
            ["Fact", "Value"],
        )
    )
    lines.append("")
    lines.append(
        f"Seed `{run.get('seed')}`, {run.get('repetitions')} repetitions per side, "
        f"{run.get('warmups')} warmup(s), budget {run.get('budget_seconds')}s, "
        f"elapsed {run.get('elapsed_seconds', 0):.1f}s, "
        f"processed {run.get('processed_tasks')}/{run.get('task_count')} timed tasks, "
        f"truncated={run.get('truncated')}."
    )
    lines.append("")

    gate_rows = []
    diagnostic_rows = []
    other_rows = []
    for workload in run.get("workloads", []):
        timing = workload.get("timing") or {}
        if timing:
            ratio = timing.get("median_ratio_oracle_over_rust")
            ci = timing.get("median_ratio_ci95") or [None, None]
            row = [
                f"`{workload['id']}`",
                workload["class"],
                workload["equivalence"],
                f"{ratio:.2f}×" if ratio is not None else "—",
                f"{ci[0]:.2f}–{ci[1]:.2f}" if ci[0] is not None else "—",
                _fmt_s(timing["rust"]["median_s"]),
                _fmt_s(timing["oracle"]["median_s"]),
                f"{timing['rust'].get('n')}/{timing['oracle'].get('n')}",
                timing.get("noise", "—"),
                workload["status"],
            ]
            if workload.get("gate_eligible") and workload["class"] == "work" and workload["equivalence"] == "strict":
                gate_rows.append(row)
            elif workload["class"] == "startup":
                other_rows.append(row)
            else:
                diagnostic_rows.append(row)
        elif workload["status"] == "pending":
            other_rows.append(
                [
                    f"`{workload['id']}`",
                    "pending",
                    workload["equivalence"],
                    "—",
                    "—",
                    "—",
                    "—",
                    "—",
                    "—",
                    workload.get("reason", "")[:90],
                ]
            )
        else:
            other_rows.append(
                [
                    f"`{workload['id']}`",
                    workload["class"],
                    workload["equivalence"],
                    "—",
                    "—",
                    "—",
                    "—",
                    "—",
                    "—",
                    f"{workload['status']}: {workload.get('reason','')}"[:120],
                ]
            )

    headers = ["Workload", "Class", "Equivalence", "Ratio", "95% CI", "Rust median", "Python median", "n R/P", "Noise", "Status"]
    lines.append("## Gate-eligible strict workloads")
    lines.append("")
    if gate_rows:
        lines.append(_table(gate_rows, headers))
        lines.append("")
        geomean = gate.get("geomean_ratio")
        lines.append(
            f"Geometric mean over {gate.get('gate_workload_count')} strict workload(s): "
            f"**{geomean:.3f}×** (target ≥ 3.0×)." if geomean is not None else "No geomean (no ratios)."
        )
    else:
        lines.append("None measured. A partial benchmark cannot satisfy the performance gate.")
    lines.append("")

    lines.append("## Diagnostics (not gate evidence)")
    lines.append("")
    if diagnostic_rows:
        lines.append(_table(diagnostic_rows, headers))
        lines.append("")
        lines.append(
            "Projection pairs compare only facts both implementations expose; micro pairs are "
            "launch-dominated. Neither is allowed to stand in for a required workload."
        )
    else:
        lines.append("None selected.")
    lines.append("")

    lines.append("## Startup and pending work")
    lines.append("")
    if other_rows:
        lines.append(_table(other_rows, headers))
    else:
        lines.append("None.")
    lines.append("")

    lines.append("## Fixtures")
    lines.append("")
    fixture_rows = []
    for name, fixture in fixtures.items():
        fixture_rows.append(
            [
                f"`{name}`",
                str(fixture.get("file_count")),
                fixture.get("manifest_sha256", "")[:32],
                json.dumps(fixture.get("recipe", {}), sort_keys=True),
            ]
        )
    lines.append(_table(fixture_rows, ["Fixture", "Files", "Manifest sha256", "Recipe"]))
    lines.append("")

    lines.append("## Method and limits")
    lines.append("")
    lines.append(
        "- Fixtures are generated deterministically from the recorded seed; inputs are never copied "
        "from a developer working tree and are never mutated."
    )
    lines.append(
        "- Equivalence is asserted on a preflight run *before* any timing, then re-asserted for every "
        "sample by hashing the normalised payload. A payload that drifts invalidates the sample."
    )
    lines.append(
        "- Both sides run in randomised, interleaved order. Raw samples include child CPU time, "
        "exit status, order index, and load before/after; medians, CV, MAD, outliers, and a seeded "
        "bootstrap 95% ratio CI are reported."
    )
    lines.append(
        "- User state is isolated (HOME/XDG/TMPDIR under the run directory). Python bytecode is written "
        "to a run-local cache, so `<repo>/src`, the installed tool environment, and real user state are "
        "not modified."
    )
    lines.append(
        "- The Python side is the pinned reference executed with `PYTHONPATH=<repo>/src`; when no "
        "equivalent CLI command exists, a development-only adapter calls the upstream library once per "
        "process. Adapters are oracle tooling, never a production fallback."
    )
    lines.append(
        "- Floor controls (Rust `--version`, Python import-only adapter) decompose total latency into "
        "protocol/launch cost and work excess. The gate uses total end-to-end medians; the work-excess "
        "ratio is reported as a diagnostic."
    )
    lines.append(
        "- This is a shared VM. Load is recorded, samples are bounded, and a high-variance or "
        "truncated run blocks the gate rather than being rounded into a pass."
    )
    lines.append("")
    return "\n".join(lines)


def write_reports(
    *,
    out_dir: Path,
    run: dict,
    identity: dict,
    gate: dict,
    fixtures: dict,
    packs_root: str | None,
) -> dict[str, Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    paths = {}
    report = {
        "schema": "spkbench.report.v1",
        "run": run,
        "identity": identity,
        "gate": gate,
        "fixtures": fixtures,
        "packs_root": packs_root,
    }
    paths["report_json"] = out_dir / "report.json"
    paths["report_json"].write_text(json.dumps(report, indent=2, sort_keys=True), encoding="utf-8")
    paths["identity_json"] = out_dir / "identity.json"
    paths["identity_json"].write_text(json.dumps(identity, indent=2, sort_keys=True), encoding="utf-8")
    paths["environment_json"] = out_dir / "environment.json"
    paths["environment_json"].write_text(
        json.dumps({"machine": identity.get("machine"), "run_load": run.get("load")}, indent=2, sort_keys=True),
        encoding="utf-8",
    )
    paths["fixtures_json"] = out_dir / "fixtures.json"
    paths["fixtures_json"].write_text(json.dumps(fixtures, indent=2, sort_keys=True), encoding="utf-8")
    paths["samples_jsonl"] = out_dir / "samples.jsonl"
    with paths["samples_jsonl"].open("w", encoding="utf-8") as handle:
        for workload in run.get("workloads", []):
            samples = workload.get("samples") or {}
            for side in ("rust", "oracle"):
                for sample in samples.get(side, []):
                    handle.write(
                        json.dumps(
                            {"workload": workload["id"], "side": side, **sample}, sort_keys=True
                        )
                        + "\n"
                    )
    paths["summary_md"] = out_dir / "summary.md"
    paths["summary_md"].write_text(
        render_markdown(run=run, identity=identity, gate=gate, fixtures=fixtures, packs_root=packs_root),
        encoding="utf-8",
    )
    return paths
