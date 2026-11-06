"""Pinned-Source write-observer oracle (development evidence only, never production).

Runs the real `kernel.atomic` observer protocol and the real notifying/not-notifying
writers under an isolated root and prints one JSON document of observed outcomes.
Every assertion below is the *intended Source cause*; the Rust test asserts this
document first, then runs the equivalent Native child.
"""

from __future__ import annotations

import asyncio
import contextvars
import json
import os
import sys
import threading
from pathlib import Path

ROOT = Path(os.environ["SK_WRITE_OBSERVER_ROOT"])

EXPECTED_ROOTS = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
]


def assert_roots() -> None:
    assert sys.version_info[:3] == (3, 11, 15), sys.version_info
    assert Path.cwd() == ROOT, Path.cwd()
    for key, name in EXPECTED_ROOTS:
        assert Path(os.environ[key]) == ROOT / name, (key, os.environ[key])
    repo = Path(__file__).resolve().parents[4]
    assert repo.is_absolute() and Path(os.environ["PYTHONPATH"]) == repo / "src"
    assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"


def scenario_no_observer() -> dict:
    from kernel.atomic import notify_written

    calls = []
    token = None
    notify_written(Path("does/not/exist.txt"), b"\x00\x01arbitrary")
    notify_written(Path("nul\x00name"), b"")
    assert calls == []
    # install/stop must not create or touch anything
    before = sorted(p.name for p in ROOT.iterdir())
    from kernel.atomic import observe_writes, stop_observing_writes

    token = observe_writes(lambda path, content: calls.append([str(path), list(content)]))
    stop_observing_writes(token)
    after = sorted(p.name for p in ROOT.iterdir())
    assert before == after, (before, after)
    notify_written(Path("still/nothing"), b"x")
    assert calls == [], calls
    return {"no_observer_noop": True, "install_stop_no_fs": before == after}


def scenario_direct_notify_path() -> dict:
    from kernel.atomic import notify_written, observe_writes, stop_observing_writes

    calls = []
    ctx = contextvars.Context()

    def encoded(path: Path) -> str:
        return os.fsencode(os.fspath(path)).hex()

    def run() -> None:
        token = observe_writes(lambda path, content: calls.append([encoded(path), list(content)]))
        try:
            notify_written(Path("relative/../weird"), b"\xff\x00\xfe")
            notify_written(Path("nul\x00inside"), b"")
            notify_written(Path(os.fsdecode(b"non\xffutf8")), b"bytes")
        finally:
            stop_observing_writes(token)

    ctx.run(run)
    assert calls == [
        ["72656c61746976652f2e2e2f7765697264", [255, 0, 254]],
        ["6e756c00696e73696465", []],
        ["6e6f6eff75746638", list(b"bytes")],
    ], calls
    return {"paths": calls}


def scenario_chain_and_error() -> dict:
    from kernel.atomic import notify_written, observe_writes, stop_observing_writes

    result = {}
    ctx = contextvars.Context()

    def run() -> None:
        order = []

        def outer(_path, _content):
            order.append("outer")

        def inner(_path, _content):
            order.append("inner")

        to_outer = observe_writes(outer)
        to_inner = observe_writes(inner)
        notify_written(Path("chain"), b"1")
        assert order == ["outer", "inner"], order
        order.clear()
        stop_observing_writes(to_inner)
        notify_written(Path("chain"), b"2")
        assert order == ["outer"], order
        stop_observing_writes(to_outer)
        order.clear()
        notify_written(Path("chain"), b"3")
        assert order == [], order

        def boom(_path, _content):
            order.append("boom")
            raise ValueError("observer refused")

        to_boom = observe_writes(boom)
        to_later = observe_writes(lambda _path, _content: order.append("later"))
        try:
            notify_written(Path("chain"), b"4")
        except ValueError as error:
            result["error_class"] = type(error).__name__
            result["error_message"] = str(error)
        else:
            raise AssertionError("observer did not raise")
        assert order == ["boom"], order
        stop_observing_writes(to_later)
        stop_observing_writes(to_boom)

    ctx.run(run)
    assert result == {"error_class": "ValueError", "error_message": "observer refused"}, result
    return result


def scenario_out_of_order() -> dict:
    from kernel.atomic import notify_written, observe_writes, stop_observing_writes

    seen = []
    ctx = contextvars.Context()

    def run() -> None:
        to_outer = observe_writes(lambda _path, _content: seen.append("outer"))
        to_inner = observe_writes(lambda _path, _content: seen.append("inner"))
        notify_written(Path("o"), b"1")
        assert seen == ["outer", "inner"], seen
        seen.clear()
        stop_observing_writes(to_outer)  # out of order: drops the inner observer
        notify_written(Path("o"), b"2")
        assert seen == [], seen
        stop_observing_writes(to_inner)  # resurrects the captured outer chain
        notify_written(Path("o"), b"3")
        assert seen == ["outer"], seen

    ctx.run(run)
    return {"order": seen, "resurrected": seen == ["outer"]}


def scenario_token_classes() -> dict:
    from kernel.atomic import observe_writes, stop_observing_writes

    out = {}
    ctx = contextvars.Context()
    holder = {}

    def run() -> None:
        to_first = observe_writes(lambda _path, _content: None)
        stop_observing_writes(to_first)
        try:
            stop_observing_writes(to_first)
        except RuntimeError as error:
            out["reused_class"] = type(error).__name__
            out["reused_message"] = str(error)
        else:
            raise AssertionError("reused token did not raise")
        holder["token"] = to_first

    foreign = contextvars.Context()
    foreign_token = foreign.run(lambda: observe_writes(lambda _path, _content: None))

    def make_foreign() -> None:
        pass

    # A token created inside `foreign` is reset from the main context.
    holder["foreign"] = foreign_token
    try:
        stop_observing_writes(foreign_token)
    except ValueError as error:
        out["foreign_class"] = type(error).__name__
        out["foreign_message"] = str(error)
    else:
        raise AssertionError("foreign token did not raise")
    # ... and it is still usable inside its own context afterwards.
    foreign.run(lambda: stop_observing_writes(foreign_token))
    out["foreign_token_unused_before_reset"] = True
    ctx.run(run)
    return out


def scenario_context_entry() -> dict:
    out = {}
    recursive = contextvars.Context()

    def inner() -> None:
        pass

    def outer() -> None:
        try:
            recursive.run(inner)
        except RuntimeError as error:
            out["recursive_class"] = type(error).__name__
            out["recursive_prefix"] = str(error).split(":")[0]
        else:
            raise AssertionError("recursive context entry did not raise")

    recursive.run(outer)

    concurrent = contextvars.Context()
    entered = threading.Event()
    release = threading.Event()

    def hold() -> None:
        def body() -> None:
            entered.set()
            release.wait(10)
            release.clear()

        concurrent.run(body)

    thread = threading.Thread(target=hold)
    thread.start()
    assert entered.wait(10), "holder thread did not start"
    try:
        concurrent.run(lambda: None)
    except RuntimeError as error:
        out["concurrent_class"] = type(error).__name__
        out["concurrent_prefix"] = str(error).split(":")[0]
    else:
        raise AssertionError("concurrent context entry did not raise")
    finally:
        release.set()
        thread.join(10)
    assert not thread.is_alive()
    return out


def scenario_copy() -> dict:
    from kernel.atomic import notify_written, observe_writes, stop_observing_writes

    seen = []
    out = {}
    original = contextvars.Context()
    token = original.run(lambda: observe_writes(lambda _path, _content: seen.append("outer")))
    copy = original.copy()

    def notify(tag: str) -> None:
        notify_written(Path(tag), tag.encode())

    copy.run(lambda: notify("copy"))
    original.run(lambda: notify("original"))
    assert seen == ["outer", "outer"], seen
    seen.clear()

    # Installing on the copy is isolated from the original binding.
    copy.run(lambda: observe_writes(lambda _path, _content: seen.append("copy-only")))
    copy.run(lambda: notify("copy"))
    original.run(lambda: notify("original"))
    out["copy_order"] = list(seen)
    seen.clear()

    # The copy shares the callback objects (their captured state), not the binding.
    original.run(lambda: notify("original"))
    out["original_after_copy_install"] = list(seen)
    original.run(lambda: stop_observing_writes(token))
    out["shared_state"] = True
    return out


def scenario_threads() -> dict:
    from kernel.atomic import notify_written, observe_writes, stop_observing_writes

    seen = []
    out = {}
    captured = contextvars.Context()
    token = captured.run(lambda: observe_writes(lambda _path, _content: seen.append("captured")))
    plain = threading.Thread(target=lambda: notify_written(Path("plain"), b"plain") or seen.append("plain-thread"))
    plain.start()
    plain.join(10)
    assert not plain.is_alive()
    assert "plain-thread" in seen and "captured" not in seen, seen
    seen.clear()

    copied = threading.Thread(target=lambda: captured.run(lambda: notify_written(Path("t"), b"t")))
    copied.start()
    copied.join(10)
    assert not copied.is_alive()
    out["copied_thread_heard"] = seen == ["captured"]
    captured.run(lambda: stop_observing_writes(token))
    return out


def scenario_asyncio() -> dict:
    from kernel.atomic import notify_written, observe_writes, stop_observing_writes

    async def main() -> dict:
        seen_a = []
        seen_b = []
        gate = asyncio.Event()

        async def task(name: str) -> None:
            await gate.wait()
            notify_written(Path(name), name.encode())

        ctx_a = contextvars.Context()
        token_a = ctx_a.run(lambda: observe_writes(lambda path, _content: seen_a.append(str(path))))
        ctx_b = contextvars.Context()
        token_b = ctx_b.run(lambda: observe_writes(lambda path, _content: seen_b.append(str(path))))
        task_a = asyncio.Task(task("a"), context=ctx_a)
        task_b = asyncio.Task(task("b"), context=ctx_b)
        gate.set()
        await asyncio.gather(task_a, task_b)
        ctx_a.run(lambda: stop_observing_writes(token_a))
        ctx_b.run(lambda: stop_observing_writes(token_b))
        return {"task_a": seen_a, "task_b": seen_b}

    result = asyncio.run(main())
    assert result["task_a"] == ["a"], result
    assert result["task_b"] == ["b"], result
    return result


def scenario_writers() -> dict:
    from kernel.atomic import atomic_write, observe_writes, stop_observing_writes
    from specify_cli.runtime.generated_writer import write_generated_file
    from specify_cli.status.views import _atomic_write_json

    out = {}
    ctx = contextvars.Context()

    def run() -> None:
        base = ROOT / "writers"
        base.mkdir()
        atomic_path = base / "atomic.txt"
        atomic_path.write_bytes(b"old")
        atomic_path.chmod(0o444)
        seen = []
        token = observe_writes(lambda path, content: seen.append([path.name, list(content)]))
        atomic_write(atomic_path, b"new")
        assert seen == [[atomic_path.name, list(b"new")]], seen
        assert atomic_path.read_bytes() == b"new"
        assert atomic_path.stat().st_mode & 0o777 == 0o444
        seen.clear()

        def boom(_path, _content):
            raise ValueError("ledger refused")

        token_boom = observe_writes(boom)
        try:
            atomic_write(atomic_path, b"later")
        except ValueError as error:
            out["atomic_error_class"] = type(error).__name__
        else:
            raise AssertionError("atomic observer did not raise")
        assert atomic_path.read_bytes() == b"later"
        assert atomic_path.stat().st_mode & 0o777 == 0o444
        assert list(base.iterdir()) == [atomic_path], list(base.iterdir())
        stop_observing_writes(token_boom)

        # DerivedJson notifies with the exact rendered bytes.
        derived_path = base / "status.json"
        seen.clear()
        _atomic_write_json(derived_path, {"b": 1, "a": 2})
        expected = (json.dumps({"a": 2, "b": 1}, sort_keys=True, indent=2, ensure_ascii=False) + "\n").encode()
        assert seen == [[derived_path.name, list(expected)]], seen
        out["derived_json_bytes"] = seen[0][1]

        # Generated and TextTruncate never notify.
        seen.clear()
        generated = base / "generated.txt"
        write_generated_file(generated, "generated", read_only=False)
        text = base / "plain.txt"
        text.write_text("plain")
        atomic_write(base / "atomic2.txt", b"x")
        stop_observing_writes(token)
        out["not_notifying_after_atomic_only"] = seen
        assert seen == [["atomic2.txt", list(b"x")]], seen

    ctx.run(run)
    import kernel.atomic as _atomic

    out["outside_ctx_no_listener"] = _atomic._WRITE_OBSERVER.get() is None
    assert out["outside_ctx_no_listener"]
    return out


def main() -> int:
    assert_roots()
    document = {
        "no_observer": scenario_no_observer(),
        "direct_notify": scenario_direct_notify_path(),
        "chain": scenario_chain_and_error(),
        "out_of_order": scenario_out_of_order(),
        "token_classes": scenario_token_classes(),
        "context_entry": scenario_context_entry(),
        "copy": scenario_copy(),
        "threads": scenario_threads(),
        "asyncio": scenario_asyncio(),
        "writers": scenario_writers(),
    }
    print(json.dumps(document, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
