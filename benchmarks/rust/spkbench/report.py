"""Evidence bundle writing.

Layout of ``benchmarks/rust/evidence/<run>/``::

    manifest.json              hashes of every artifact (written last)
    performance-evidence.json  checker-shaped, self-hashed bundle
    report.json                human/audit report (run + identity + gate)
    samples.jsonl              one raw row per timed invocation
    identity.json              source/binary/toolchain/oracle identity
    environment.json           machine + load context
    fixtures.json              portable per-file digests for every fixture
    payloads/*.json            content-addressed normalized preflight payloads
    post-state/*.json          post-state envelopes (manifests + delta + stored bytes)
    summary.md                 rendered report

Nothing here trusts the run: the caller re-verifies the written bundle from
disk before reporting a verdict.
"""

from __future__ import annotations

import base64
import hashlib
import json
from pathlib import Path

from . import certify
from .evidence import POST_STATE_SCHEMA, canonical_sha256

MAX_PAYLOAD_BYTES = 512 * 1024


def _post_state_envelope(record: dict) -> dict:
    """Wrap one side's post-state observation into its stored document."""
    files: dict[str, dict] = {}
    for rel, encoded in (record.get("required_files") or {}).items():
        if not isinstance(encoded, str):
            continue
        try:
            data = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError):
            continue
        entry: dict = {
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),  # noqa: TID251 - content hash
            "content_b64": encoded,
        }
        try:
            entry["canonical_sha256"] = canonical_sha256(json.loads(data.decode("utf-8")))
        except (UnicodeDecodeError, ValueError):
            pass
        files[rel] = entry
    delta = record.get("delta")
    content_roots = record.get("content_roots")
    return {
        "schema": POST_STATE_SCHEMA,
        "delta": delta if isinstance(delta, dict) else {},
        "before": record.get("before") if isinstance(record.get("before"), dict) else {},
        "after": record.get("after") if isinstance(record.get("after"), dict) else {},
        "params": record.get("params") if isinstance(record.get("params"), dict) else {},
        "content_roots": [str(item) for item in content_roots]
        if isinstance(content_roots, list)
        else [],
        "files": files,
    }


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()  # noqa: TID251 - file-integrity check
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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
    registry: dict,
    bundle_path: str,
    packs_root: str | None,
) -> str:
    machine = identity.get("machine", {})
    platform = machine.get("platform", {})
    toolchains = machine.get("toolchains", {})
    candidate = {
        "commit": (identity.get("worktree") or {}).get("head"),
        "clean": not (identity.get("worktree") or {}).get("dirty"),
    }
    toolchain = (identity.get("cargo") or {}).get("toolchain") or {}
    binaries = (identity.get("binary_provenance") or {}).get("binaries") or {}
    baseline = (identity.get("oracle") or {}).get("baseline") or {}
    lines: list[str] = []
    lines.append("# Spec Kitty Rust vs Python performance evidence")
    lines.append("")
    lines.append(f"**Gate verdict: `{gate['verdict']}`**")
    lines.append("")
    for reason in gate.get("reasons", []):
        lines.append(f"- {reason}")
    lines.append("")
    lines.append(
        "> This file describes one measurement bundle. A verdict other than `PASS` is not a "
        "performance claim; `PASS` requires the frozen 20-workload floor, 20 paired samples per "
        "side, work-excess ≥3x, a 95% aggregate CI above 3x, no statistically material >10% "
        "regression and a live-recomputable identity/build binding."
    )
    lines.append("")

    lines.append("## Identity")
    lines.append("")
    rows = [
        ["Registry", f"`{registry.get('path')}`"],
        ["Registry digest", f"`{registry.get('digest')}`"],
        ["Candidate commit", f"`{candidate['commit']}`"],
        ["Source clean", str(candidate["clean"])],
        ["Rust build-input digest", str((identity.get("cargo") or {}).get("build_inputs", {}).get("digest"))],
        ["Cargo.lock sha256", str((identity.get("cargo") or {}).get("lock_sha256"))],
        ["uv.lock sha256", str(identity.get("uv_lock_sha256"))],
        ["Build receipt", str(((identity.get("build_receipt") or {}).get("verification") or {}).get("ok"))],
        ["Toolchain", str(toolchain.get("release"))],
        ["Oracle baseline", f"`{baseline.get('baseline')}` (src matches: {baseline.get('src_matches_baseline')})"],
        ["Oracle verified", str(((identity.get("oracle") or {}).get("verify") or {}).get("ok"))],
        ["Packs digest", str((identity.get("packs") or {}).get("digest"))],
        ["Packs root", packs_root or "—"],
        ["Identity status", str(identity.get("status"))],
    ]
    for name, record in binaries.items():
        rows.append([f"Binary {name}", f"`{str(record.get('sha256'))[:32]}…` predates source: {record.get('predates_source')}"])
    lines.append(_table(rows, ["Fact", "Value"]))
    lines.append("")

    lines.append("## Measurement context")
    lines.append("")
    load = (run.get("load") or {}).get("loadavg", {})
    lines.append(
        _table(
            [
                ["Host", str(machine.get("hostname"))],
                ["CPU", f"{platform.get('cpu_model')} × {platform.get('cpu_count')}"],
                ["OS", f"{platform.get('system')} {platform.get('release')}"],
                ["Rust toolchain", (toolchains.get("rustc") or {}).get("stdout", "").splitlines()[0]
                 if (toolchains.get("rustc") or {}).get("stdout") else "—"],
                ["Oracle Python", (toolchains.get("oracle_python") or {}).get("stdout", "").splitlines()[0]
                 if (toolchains.get("oracle_python") or {}).get("stdout") else "—"],
                ["Load at end", json.dumps(load)],
            ],
            ["Fact", "Value"],
        )
    )
    lines.append("")
    lines.append(
        f"Seed `{run.get('seed')}`, {run.get('repetitions')} paired repetitions per side, "
        f"{run.get('warmups')} warm-up(s), budget {run.get('budget_seconds')}s, elapsed "
        f"{run.get('elapsed_seconds', 0):.1f}s, processed {run.get('processed_tasks')}/"
        f"{run.get('task_count')} timed tasks, truncated={run.get('truncated')}, pairing="
        f"`{run.get('pairing')}`."
    )
    lines.append("")

    headers = [
        "Workload", "Class", "Equiv", "Speedup", "95% CI", "Work excess", "n (P/R)",
        "Post-state", "Status",
    ]
    gate_rows = []
    other_rows = []
    for workload in run.get("workloads", []):
        timing = workload.get("timing") or {}
        wex = ((workload.get("timing") or {}).get("floor_control") or {}).get("work_excess_ratio")
        post = ((workload.get("preflight") or {}).get("post_state_sha256") or {}) if workload.get("preflight") else {}
        if timing:
            row = [
                f"`{workload['id']}`",
                workload.get("class", "—"),
                workload.get("equivalence", "—"),
                f"{timing.get('median_ratio_oracle_over_rust', 0):.2f}×",
                "—",
                f"{wex:.2f}×" if wex else "—",
                f"{len((workload.get('samples') or {}).get('rust') or [])}/{len((workload.get('samples') or {}).get('python') or [])}",
                ("equal" if post.get("rust") and post.get("rust") == post.get("python") else "n/a"),
                workload.get("status", "—"),
            ]
        else:
            row = [
                f"`{workload['id']}`",
                workload.get("class", "—"),
                workload.get("equivalence", "—"),
                "—",
                "—",
                "—",
                "0/0",
                "—",
                f"{workload.get('status')}: {str(workload.get('reason') or '')[:70]}",
            ]
        if workload.get("source") == "registry":
            gate_rows.append(row)
        else:
            other_rows.append(row)

    lines.append("## Frozen completion workloads")
    lines.append("")
    if gate_rows:
        lines.append(_table(gate_rows, headers))
    else:
        lines.append("None.")
    lines.append("")
    lines.append(f"Bundle: `{bundle_path}`")
    lines.append("")

    lines.append("## Diagnostics (never gate evidence)")
    lines.append("")
    if other_rows:
        lines.append(_table(other_rows, headers))
    else:
        lines.append("None selected.")
    lines.append("")

    lines.append("## Fixtures")
    lines.append("")
    fixture_rows = []
    for name, fixture in fixtures.items():
        fixture_rows.append(
            [
                f"`{name}`",
                str(fixture.get("file_count")),
                f"`{str(fixture.get('content_digest'))[:32]}…`",
                str(fixture.get("portable")),
                json.dumps(fixture.get("recipe", {}), sort_keys=True)[:160],
            ]
        )
    lines.append(_table(fixture_rows, ["Fixture", "Files", "Portable content digest", "Portable", "Recipe"]))
    lines.append("")

    lines.append("## Method")
    lines.append("")
    lines.append(
        "- Workload ids, equivalence classes, payload/state schemas and floors come from the frozen "
        "registry shared with `scripts/rust/check_completion.py`. A filtered run cannot pass."
    )
    lines.append(
        "- Equivalence is asserted on a preflight run before any timing, then re-asserted for every "
        "sample by hashing the normalized payload (and the post-state delta for mutations). A "
        "payload/schema drift invalidates the sample."
    )
    lines.append(
        "- Python and Rust run adjacent in each repetition, in randomized order. Uncertainty uses a "
        "paired bootstrap of per-pair ratios; the aggregate CI resamples pairs within every workload."
    )
    lines.append(
        "- Mutations run in a fresh fixture clone per invocation; clone and post-state scan are "
        "outside the timed window. Only declared implementation-private lock artifacts are excluded, "
        "and they are captured per invocation rather than normalised away."
    )
    lines.append(
        "- Fixtures are generated from recipes + seed, never copied from a working tree, and their "
        "digests are root-canonicalised so they can be regenerated at any audit path."
    )
    lines.append(
        "- User state is isolated (HOME/XDG/TMPDIR/PYCACHE under the run directory). The repository "
        "`src` tree and the real user state are never written."
    )
    lines.append("")
    return "\n".join(lines)


def _write_payloads(
    out_dir: Path, run: dict
) -> tuple[dict[str, dict], dict[str, dict]]:
    """Store normalized payloads and mutation post-state deltas.

    A payload file may be re-serialized by a reader (pretty JSON is fine): its
    declared ``canonical_sha256`` is the hash of the canonical JSON of the
    *parsed* document, and ``sha256`` is the hash of the bytes on disk. Both are
    recorded so a reformat is distinguishable from an edit.
    """
    payload_records: dict[str, dict] = {}
    post_state_records: dict[str, dict] = {}

    def store(key: str, value: object, *, subdir: str, records: dict[str, dict], label: str) -> None:
        canonical = certify.canonical_json(value).encode("utf-8")
        pretty = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")
        canonical_sha = hashlib.sha256(canonical).hexdigest()  # noqa: TID251 - canonical content hash
        if len(pretty) > MAX_PAYLOAD_BYTES:
            records[key] = {
                "sha256": hashlib.sha256(pretty).hexdigest(),  # noqa: TID251
                "canonical_sha256": canonical_sha,
                "bytes": len(pretty),
                "stored": False,
                "kind": label,
            }
            return
        directory = out_dir / subdir
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / f"{key}.json"
        path.write_bytes(pretty)
        records[key] = {
            "sha256": hashlib.sha256(pretty).hexdigest(),  # noqa: TID251
            "canonical_sha256": canonical_sha,
            "bytes": len(pretty),
            "stored": True,
            "path": f"{subdir}/{path.name}",
            "kind": label,
        }

    for workload in run.get("workloads", []):
        preflight = workload.get("preflight") or {}
        for side in ("rust", "python"):
            payload = (preflight.get("payload") or {}).get(side)
            if payload is not None:
                store(
                    f"{workload['id']}.{side}",
                    payload,
                    subdir="payloads",
                    records=payload_records,
                    label="normalized-payload",
                )
            if workload.get("measurement") == "mutation":
                post_state = (preflight.get("post_state") or {}).get(side)
                delta = post_state.get("delta") if isinstance(post_state, dict) else None
                if isinstance(delta, dict) and delta:
                    store(
                        f"{workload['id']}.{side}",
                        _post_state_envelope(post_state),
                        subdir="post-state",
                        records=post_state_records,
                        label="post-state-envelope",
                    )
    return payload_records, post_state_records


def write_evidence(
    *,
    out_dir: Path,
    run: dict,
    identity: dict,
    fixtures: dict,
    registry,
    harness: dict,
    repo: Path,
    packs_root: str | None,
    live_probe=None,
    verify_fixtures: bool,
    fixture_observer=None,
    live_identity: dict | None = None,  # accepted for compatibility; never trusted
) -> dict:
    """Write every artifact, then verify the written evidence from disk.

    Verification happens twice on purpose: a provisional pass once the
    measurement artifacts exist (so the rendered report can state the verdict),
    and an authoritative pass on the final bytes including ``manifest.json``.
    The two verdicts must agree, otherwise the write is refused.

    Returns ``{"paths", "bundle", "verification"}``. The returned verification is
    recomputed from the written bytes, never from memory.
    """
    out_dir.mkdir(parents=True, exist_ok=True)
    rel_dir = out_dir
    try:
        rel_dir = out_dir.resolve().relative_to(repo.resolve())
    except ValueError:
        rel_dir = out_dir
    bundle_rel = (rel_dir / "performance-evidence.json").as_posix()
    del live_identity  # a recorded identity is data the bundle asserts; the probe observes
    paths: dict[str, Path] = {}
    probe = live_probe or certify.NATIVE_PROBE_FACTORY(Path(repo))

    # --- raw samples
    paths["samples_jsonl"] = out_dir / "samples.jsonl"
    with paths["samples_jsonl"].open("w", encoding="utf-8") as handle:
        for workload in run.get("workloads", []):
            samples = workload.get("samples") or {}
            for side in ("rust", "python"):
                for sample in samples.get(side, []):
                    handle.write(
                        json.dumps({"workload": workload["id"], **sample}, sort_keys=True) + "\n"
                    )

    # --- normalized payloads and mutation post-state deltas
    payload_records, post_state_records = _write_payloads(out_dir, run)

    paths["fixtures_json"] = out_dir / "fixtures.json"
    paths["fixtures_json"].write_text(
        json.dumps(fixtures, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    paths["identity_json"] = out_dir / "identity.json"
    paths["identity_json"].write_text(
        json.dumps(identity, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    paths["environment_json"] = out_dir / "environment.json"
    paths["environment_json"].write_text(
        json.dumps(
            {"machine": identity.get("machine"), "run_load": run.get("load")},
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )

    bundle = certify.certify_run(
        run=run,
        identity=identity,
        fixtures=fixtures,
        registry=registry,
        repo=repo,
        bundle_path_rel=bundle_rel,
        harness=harness,
    )
    bundle["payload_artifacts"] = payload_records
    bundle["post_state_artifacts"] = post_state_records
    # The raw/measurement artifacts exist now; report and summary are written
    # once the verdict they state is known.
    bundle["artifacts"] = {
        name: {"path": path.name, "sha256": _sha256_file(path)}
        for name, path in (
            ("samples", paths["samples_jsonl"]),
            ("fixtures", paths["fixtures_json"]),
            ("identity", paths["identity_json"]),
            ("environment", paths["environment_json"]),
        )
    }
    bundle["self_sha256"] = certify.self_digest(bundle)
    paths["bundle_json"] = certify.write_bundle(bundle, out_dir)

    # --- provisional verdict: measurement artifacts exist, the report/summary
    # and manifest that will record the verdict do not yet.
    provisional = certify.evaluate_bundle(
        certify.load_bundle(paths["bundle_json"]),
        registry,
        repo=repo,
        live_probe=probe,
        verify_fixtures=verify_fixtures,
        fixture_observer=fixture_observer,
        artifacts_root=out_dir,
        require_artifacts=False,
        require_manifest=False,
    )
    bundle["gate"] = provisional["gate"]
    bundle["aggregate"]["status"] = (
        "pass"
        if provisional["gate"]["verdict"] == "PASS"
        else provisional["gate"]["verdict"].lower()
    )

    paths["report_json"] = out_dir / "report.json"
    paths["report_json"].write_text(
        json.dumps(
            {
                "schema": certify.REPORT_SCHEMA,
                "run": run,
                "identity": identity,
                "gate": provisional["gate"],
                "fixtures": fixtures,
                "registry": registry.identity(),
                "payload_artifacts": payload_records,
                "post_state_artifacts": post_state_records,
                "bundle_verification": {
                    "fixtures": provisional["fixtures"],
                    "recomputed": provisional["recomputed"],
                },
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )

    paths["summary_md"] = out_dir / "summary.md"
    paths["summary_md"].write_text(
        render_markdown(
            run=run,
            identity=identity,
            gate=provisional["gate"],
            fixtures=fixtures,
            registry=registry.identity(),
            bundle_path=bundle_rel,
            packs_root=packs_root,
        ),
        encoding="utf-8",
    )

    # --- artifact hashes, then the self-hashed bundle
    artifact_paths = {
        "report": paths["report_json"],
        "samples": paths["samples_jsonl"],
        "identity": paths["identity_json"],
        "environment": paths["environment_json"],
        "fixtures": paths["fixtures_json"],
        "summary": paths["summary_md"],
    }
    bundle["artifacts"] = {
        name: {"path": path.name, "sha256": _sha256_file(path)}
        for name, path in artifact_paths.items()
    }
    bundle["self_sha256"] = certify.self_digest(bundle)
    certify.write_bundle(bundle, out_dir)

    # --- manifest (does not include itself)
    manifest = {
        "schema": "spkbench.bundle-manifest.v1",
        "bundle": {
            "path": "performance-evidence.json",
            "sha256": _sha256_file(paths["bundle_json"]),
        },
        "artifacts": bundle["artifacts"],
        "payloads": payload_records,
        "post_state": post_state_records,
    }
    paths["manifest_json"] = out_dir / "manifest.json"
    paths["manifest_json"].write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    # --- authoritative re-verification from the final bytes
    written = certify.load_bundle(paths["bundle_json"])
    verification = certify.evaluate_bundle(
        written,
        registry,
        repo=repo,
        live_probe=probe,
        verify_fixtures=verify_fixtures,
        fixture_observer=fixture_observer,
        artifacts_root=out_dir,
        require_artifacts=True,
        require_manifest=True,
    )
    if verification["gate"]["verdict"] != provisional["gate"]["verdict"]:
        raise RuntimeError(
            "written bundle does not reproduce the in-memory verdict: "
            f"{provisional['gate']['verdict']} != {verification['gate']['verdict']}: "
            f"{verification['gate']['reasons'][:3]}"
        )

    # The on-disk bundle must record the *authoritative* verdict, not the
    # provisional one (the two can differ only in warnings/manifest notes).
    # Rewriting the gate changes the bundle bytes, so re-seal the manifest and
    # verify the final bytes once more; the verdict must stay the same.
    expected_status = "pass" if verification["gate"]["verdict"] == "PASS" else verification[
        "gate"
    ]["verdict"].lower()
    stable = (
        written.get("gate") == verification["gate"]
        and (written.get("aggregate") or {}).get("status") == expected_status
    )
    for _attempt in range(3):
        if stable:
            break
        written["gate"] = verification["gate"]
        written["aggregate"]["status"] = expected_status
        written["self_sha256"] = certify.self_digest(written)
        certify.write_bundle(written, out_dir)
        manifest["bundle"]["sha256"] = _sha256_file(paths["bundle_json"])
        paths["manifest_json"].write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        written = certify.load_bundle(paths["bundle_json"])
        verification = certify.evaluate_bundle(
            written,
            registry,
            repo=repo,
            live_probe=probe,
            verify_fixtures=verify_fixtures,
            fixture_observer=fixture_observer,
            artifacts_root=out_dir,
            require_artifacts=True,
            require_manifest=True,
        )
        if verification["gate"]["verdict"] != provisional["gate"]["verdict"]:
            raise RuntimeError(
                "re-sealed bundle does not reproduce the verdict: "
                f"{provisional['gate']['verdict']} != {verification['gate']['verdict']}"
            )
        stable = (
            written.get("gate") == verification["gate"]
            and (written.get("aggregate") or {}).get("status") == expected_status
        )
    if not stable:
        raise RuntimeError("written bundle did not stabilise on its authoritative verdict")
    return {"paths": paths, "bundle": written, "verification": verification}
