"""Non-vacuous payload and post-state expectations.

Equivalence is only meaningful when both sides are required to produce a
*specific, non-empty* result. Comparing two empty documents, two unparseable
blobs or two ``{}`` payloads is equal but worthless, and a mutating workload that
writes nothing on either side is not equivalent work either.

A workload therefore declares a small, declarative schema in the frozen
registry. This module is the single evaluator for those schemas, used by the
preflight, by every timed sample (payload and post-state) and by the bundle
certifier. The mini-language is intentionally boring so it stays auditable:

``stdout``      required substrings/patterns/byte and line floors for contract
                workloads (startup).
``payload``     required/non-null/non-empty keys, exact dotted-path values,
                minimum collection sizes, row keys and row values.
``post_state``  required/unchanged paths, minimum file sizes, a JSON payload
                schema for the written artifact, and the exact set of allowed
                additions/changes/removals.
"""

from __future__ import annotations

import fnmatch
import hashlib
import json
import re
from pathlib import Path
from typing import Any, Iterable

from .fixtures import canonical_bytes

_STRUCTURAL_PAYLOAD_KEYS = (
    "required_keys",
    "nonnull_keys",
    "nonempty_keys",
    "equals",
    "min_items",
    "row_keys",
    "row_equals",
    "required_patterns",
)

#: A post-state manifest value is either a content digest or a symlink target.
_CONTENT_DIGEST = re.compile(r"[0-9a-f]{64}")
_SYMLINK_PREFIX = "symlink:"
_POST_STATE_CATEGORIES = ("added", "changed", "removed")


def substitute(template: str, params: dict[str, Any]) -> str:
    text = template
    for key, value in params.items():
        text = text.replace("{" + key + "}", str(value))
    return text


def lookup(document: Any, path: str) -> Any:
    """Resolve ``a.b[0].c`` against a decoded JSON document."""
    current = document
    for raw in path.split("."):
        name = raw
        index = None
        match = re.fullmatch(r"([^\[]*)\[(-?\d+)\]", raw)
        if match:
            name, index = match.group(1), int(match.group(2))
        if name:
            if not isinstance(current, dict) or name not in current:
                raise KeyError(path)
            current = current[name]
        if index is not None:
            if not isinstance(current, list) or not (-len(current) <= index < len(current)):
                raise KeyError(path)
            current = current[index]
    return current


def _rows(value: Any) -> Iterable[dict]:
    if isinstance(value, dict):
        for item in value.values():
            if isinstance(item, dict):
                yield item
    elif isinstance(value, list):
        for item in value:
            if isinstance(item, dict):
                yield item


def validate_stdout(stdout: bytes, schema: dict) -> list[str]:
    problems: list[str] = []
    text = stdout.decode("utf-8", "replace")
    min_bytes = schema.get("min_bytes")
    if min_bytes is not None and len(stdout) < int(min_bytes):
        problems.append(f"stdout is {len(stdout)} bytes, needs >= {min_bytes}")
    if not stdout.strip():
        problems.append("stdout is empty")
    min_lines = schema.get("min_lines")
    if min_lines is not None:
        lines = [line for line in text.splitlines() if line.strip()]
        if len(lines) < int(min_lines):
            problems.append(f"stdout has {len(lines)} non-blank lines, needs >= {min_lines}")
    for needle in schema.get("required_substrings") or []:
        if str(needle) not in text:
            problems.append(f"stdout does not contain {needle!r}")
    for pattern in schema.get("required_patterns") or []:
        if not re.search(str(pattern), text):
            problems.append(f"stdout does not match /{pattern}/")
    return problems


def validate_payload(payload: Any, schema: dict) -> list[str]:
    problems: list[str] = []
    if payload is None:
        return ["payload is null"]
    if isinstance(payload, dict) and "_unparseable" in payload:
        return [f"payload is unparseable: {str(payload.get('_unparseable'))[:120]}"]
    if not isinstance(payload, dict):
        return [f"payload is {type(payload).__name__}, expected an object"]

    for key in schema.get("required_keys") or []:
        if key not in payload:
            problems.append(f"payload is missing required key {key!r}")
    for key in schema.get("nonnull_keys") or []:
        try:
            if lookup(payload, key) is None:
                problems.append(f"payload key {key!r} is null")
        except KeyError:
            problems.append(f"payload key {key!r} is missing")
    for key in schema.get("nonempty_keys") or []:
        try:
            value = lookup(payload, key)
        except KeyError:
            problems.append(f"payload key {key!r} is missing")
            continue
        if not value:
            problems.append(f"payload key {key!r} is empty")
    for key, expected in (schema.get("equals") or {}).items():
        try:
            value = lookup(payload, key)
        except KeyError:
            problems.append(f"payload has no {key!r} to compare")
            continue
        if value != expected:
            problems.append(f"payload {key} is {value!r}, expected {expected!r}")
    for key, minimum in (schema.get("min_items") or {}).items():
        try:
            value = lookup(payload, key)
        except KeyError:
            problems.append(f"payload has no {key!r} to size")
            continue
        try:
            size = len(value)
        except TypeError:
            problems.append(f"payload {key} is not a collection")
            continue
        if size < int(minimum):
            problems.append(f"payload {key} has {size} items, needs >= {minimum}")
    for key, row_keys in (schema.get("row_keys") or {}).items():
        try:
            value = lookup(payload, key)
        except KeyError:
            problems.append(f"payload has no {key!r} rows")
            continue
        rows = list(_rows(value))
        if not rows:
            problems.append(f"payload {key} has no object rows")
            continue
        for index, row in enumerate(rows[:50]):
            missing = [name for name in row_keys if name not in row]
            if missing:
                problems.append(f"payload {key}[{index}] is missing {missing}")
                break
    for key, expected_fields in (schema.get("row_equals") or {}).items():
        try:
            value = lookup(payload, key)
        except KeyError:
            problems.append(f"payload has no {key!r} rows")
            continue
        for index, row in enumerate(list(_rows(value))[:200]):
            for name, expected in expected_fields.items():
                if row.get(name) != expected:
                    problems.append(
                        f"payload {key}[{index}].{name} is {row.get(name)!r}, expected {expected!r}"
                    )
                    break
    for key, pattern in (schema.get("required_patterns") or {}).items():
        try:
            value = lookup(payload, key)
        except KeyError:
            problems.append(f"payload has no {key!r} to pattern-match")
            continue
        if not re.search(str(pattern), str(value)):
            problems.append(f"payload {key}={value!r} does not match /{pattern}/")
    return problems


def _manifest_is_digest(value: Any) -> bool:
    return isinstance(value, str) and (
        bool(_CONTENT_DIGEST.fullmatch(value)) or value.startswith(_SYMLINK_PREFIX)
    )


def _manifest_problems(label: str, manifest: Any) -> list[str]:
    if not isinstance(manifest, dict):
        return [f"post-state {label} manifest is not an object"]
    problems: list[str] = []
    for rel, value in manifest.items():
        if not isinstance(rel, str) or not rel:
            problems.append(f"post-state {label} manifest has a non-path key {rel!r}")
        elif not _manifest_is_digest(value):
            problems.append(
                f"post-state {label} manifest entry {rel!r} is not a content digest"
            )
    return problems


def _delta_problems(delta: Any, before: dict[str, str], after: dict[str, str]) -> list[str]:
    """The delta must be exactly the difference between the stored manifests."""
    if not isinstance(delta, dict):
        return ["post-state delta is not an object"]
    problems: list[str] = []
    expected = {
        "added": sorted(set(after) - set(before)),
        "changed": sorted(rel for rel in set(before) & set(after) if before[rel] != after[rel]),
        "removed": sorted(set(before) - set(after)),
    }
    for category in _POST_STATE_CATEGORIES:
        recorded = delta.get(category)
        if not isinstance(recorded, list) or any(
            not isinstance(item, str) or not item for item in recorded
        ):
            problems.append(f"post-state delta {category} is not a list of paths")
            continue
        if len(set(recorded)) != len(recorded):
            problems.append(f"post-state delta {category} lists a path more than once")
        missing = sorted(set(expected[category]) - set(recorded))[:5]
        unexpected = sorted(set(recorded) - set(expected[category]))[:5]
        if missing or unexpected:
            problems.append(
                f"post-state delta {category} does not re-derive from the stored manifests "
                f"(missing {missing}, unexpected {unexpected})"
            )
    for key, category, source in (
        ("added_sha256", "added", after),
        ("removed_sha256", "removed", before),
    ):
        table = delta.get(key)
        if table is None:
            continue
        if not isinstance(table, dict):
            problems.append(f"post-state delta {key} is not an object")
            continue
        wanted = {rel: source[rel] for rel in expected[category]}
        if table != wanted:
            problems.append(f"post-state delta {key} does not match the stored manifests")
    changed_table = delta.get("changed_sha256")
    if changed_table is not None:
        wanted_changed = {
            rel: {"before": before[rel], "after": after[rel]} for rel in expected["changed"]
        }
        if changed_table != wanted_changed:
            problems.append("post-state delta changed_sha256 does not match the stored manifests")
    return problems


def required_post_state_paths(schema: dict, params: dict[str, Any]) -> list[str]:
    """Every post-state path whose actual content the schema constrains."""
    paths = [substitute(item, params) for item in schema.get("required_paths") or []]
    json_key = schema.get("json_payload_key")
    if json_key:
        paths.append(substitute(json_key, params))
    return sorted({path for path in paths if path})


def _read_post_state_file(
    rel: str, *, root: Path | None, files: dict[str, bytes] | None
) -> tuple[bool, bytes | None]:
    if files is not None:
        data = files.get(rel)
        if isinstance(data, (bytes, bytearray)):
            return True, bytes(data)
        return False, None
    if root is None:
        return False, None
    path = Path(root) / rel
    if not path.is_file():
        return False, None
    try:
        return True, path.read_bytes()
    except OSError:
        return False, None


def _required_content_problems(
    *,
    schema: dict,
    params: dict[str, Any],
    after: dict[str, str],
    root: Path | None,
    files: dict[str, bytes] | None,
    content_roots: tuple[Path, ...],
) -> list[str]:
    """Required paths must carry content that hashes to the stored manifest.

    Manifest values are portable: they are hashes of the file bytes after the
    shared fixture/clone location roots are canonicalised. The content digest
    here therefore uses the same normalisation (never a raw, location-dependent
    hash compared silently with a portable one).
    """
    problems: list[str] = []
    for rel in (substitute(item, params) for item in schema.get("required_paths") or []):
        if rel not in after:
            problems.append(f"post-state is missing required path {rel!r}")
    for rel in required_post_state_paths(schema, params):
        exists, data = _read_post_state_file(rel, root=root, files=files)
        if not exists or data is None:
            problems.append(f"post-state file {rel!r} has no stored content to validate")
            continue
        recorded = after.get(rel)
        if isinstance(recorded, str) and _CONTENT_DIGEST.fullmatch(recorded):
            actual = hashlib.sha256(  # noqa: TID251 - content binding
                canonical_bytes(data, roots=content_roots)
            ).hexdigest()
            if actual != recorded:
                problems.append(
                    f"post-state file {rel!r} content does not match its after-manifest hash"
                )
    return problems


def _size_problems(
    *,
    schema: dict,
    params: dict[str, Any],
    root: Path | None,
    files: dict[str, bytes] | None,
) -> list[str]:
    problems: list[str] = []
    for template, minimum in (schema.get("min_bytes") or {}).items():
        rel = substitute(template, params)
        exists, data = _read_post_state_file(rel, root=root, files=files)
        if not exists or data is None:
            problems.append(f"post-state file {rel!r} is missing")
            continue
        if len(data) < int(minimum):
            problems.append(f"post-state file {rel!r} is {len(data)} bytes, needs >= {minimum}")
    return problems


def _json_payload_problems(
    *,
    schema: dict,
    params: dict[str, Any],
    root: Path | None,
    files: dict[str, bytes] | None,
) -> list[str]:
    json_key = schema.get("json_payload_key")
    if not json_key:
        return []
    rel = substitute(json_key, params)
    exists, data = _read_post_state_file(rel, root=root, files=files)
    if not exists or data is None:
        return [f"post-state JSON {rel!r} is unreadable"]
    try:
        payload = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        return [f"post-state JSON {rel!r} is unreadable: {exc}"]
    return [
        f"{rel}: {problem}"
        for problem in validate_payload(payload, schema.get("json_payload") or {})
    ]


def _unchanged_problems(
    *, schema: dict, params: dict[str, Any], before: dict[str, str], after: dict[str, str]
) -> list[str]:
    problems: list[str] = []
    for template in schema.get("unchanged_paths") or []:
        rel = substitute(template, params)
        if rel not in before or rel not in after:
            problems.append(f"unchanged path {rel!r} is missing before or after")
        elif before[rel] != after[rel]:
            problems.append(f"declared read-only path {rel!r} was modified")
    return problems


def _declared_delta_problems(
    *, schema: dict, params: dict[str, Any], delta: dict
) -> list[str]:
    problems: list[str] = []
    for category, patterns in (
        ("added", schema.get("expected_additions") or []),
        ("changed", schema.get("expected_changes") or []),
        ("removed", schema.get("expected_removals") or []),
    ):
        allowed = [substitute(pattern, params) for pattern in patterns]
        label = {"added": "addition", "changed": "change", "removed": "removal"}[category]
        for path in delta.get(category, []):
            if not any(fnmatch.fnmatch(path, pattern) for pattern in allowed):
                problems.append(f"undeclared post-state {label}: {path!r}")
    return problems


def validate_post_state(
    *,
    before: dict[str, str],
    after: dict[str, str],
    delta: dict,
    schema: dict,
    params: dict[str, Any],
    root: Path | None = None,
    files: dict[str, bytes] | None = None,
    content_roots: Iterable[str | Path] = (),
) -> list[str]:
    """Assert that a mutation produced the declared, non-vacuous post-state.

    ``root`` reads the files from a live tree (the measurement side); ``files``
    reads the same paths from stored bytes (the evidence side). Either way the
    *same* schema evaluation runs: a required path that is merely named, or a
    stored delta that does not re-derive from the stored manifests, is not
    evidence of what the implementation actually wrote.
    """
    problems: list[str] = []
    roots = tuple(Path(item) for item in content_roots)
    problems.extend(_manifest_problems("before", before))
    problems.extend(_manifest_problems("after", after))
    if not problems:
        problems.extend(_delta_problems(delta, before, after))
    if root is None and files is None:
        return problems + ["post-state has no file view to validate"]

    ignored = tuple(schema.get("ignored_prefixes") or [])
    for path in delta.get("added", []) + delta.get("changed", []) + delta.get("removed", []):
        if any(path == prefix.rstrip("/") or path.startswith(prefix) for prefix in ignored):
            problems.append(f"ignored artifact {path!r} leaked into the equality manifest")

    problems.extend(
        _required_content_problems(
            schema=schema,
            params=params,
            after=after,
            root=root,
            files=files,
            content_roots=roots,
        )
    )
    problems.extend(_size_problems(schema=schema, params=params, root=root, files=files))
    problems.extend(_json_payload_problems(schema=schema, params=params, root=root, files=files))
    problems.extend(
        _unchanged_problems(schema=schema, params=params, before=before, after=after)
    )
    problems.extend(_declared_delta_problems(schema=schema, params=params, delta=delta))
    if not (schema.get("required_paths") or []):
        problems.append("post-state schema declares no required path")
    return problems


def non_vacuous(expectations: dict) -> tuple[bool, str]:
    """Reject a schema that could be satisfied by two empty/degenerate results."""
    if not expectations:
        return False, "no expectations declared"
    if expectations.get("exit") is None:
        return False, "no expected exit code"
    stdout = expectations.get("stdout") or {}
    payload = expectations.get("payload") or {}
    post = expectations.get("post_state") or {}
    stdout_structural = bool(
        stdout.get("min_bytes") or stdout.get("min_lines") or stdout.get("required_substrings")
        or stdout.get("required_patterns")
    )
    payload_structural = any(payload.get(key) for key in _STRUCTURAL_PAYLOAD_KEYS)
    post_structural = bool(post.get("required_paths") and (post.get("json_payload") or post.get("min_bytes")))
    if not (stdout_structural or payload_structural or post_structural):
        return False, "no structural requirement on stdout, payload or post-state"
    if payload_structural and not (
        payload.get("required_keys") or payload.get("equals") or payload.get("min_items")
        or payload.get("nonempty_keys") or payload.get("row_keys") or payload.get("required_patterns")
    ):
        return False, "payload schema has no key/value/size requirement"
    return True, ""


def validate_expectations(
    expectations: dict,
    *,
    stdout: bytes | None = None,
    payload: Any = None,
    post_state: dict | None = None,
) -> list[str]:
    """Apply whichever declared schemas the caller has evidence for."""
    problems: list[str] = []
    if stdout is not None:
        problems.extend(validate_stdout(stdout, expectations.get("stdout") or {}))
    if payload is not None:
        problems.extend(validate_payload(payload, expectations.get("payload") or {}))
    if post_state is not None:
        problems.extend(
            validate_post_state(
                before=post_state["before"],
                after=post_state["after"],
                delta=post_state["delta"],
                root=Path(post_state["root"]) if post_state.get("root") is not None else None,
                files=post_state.get("files"),
                content_roots=post_state.get("content_roots") or (),
                schema=expectations.get("post_state") or {},
                params=post_state.get("params") or {},
            )
        )
    return problems
