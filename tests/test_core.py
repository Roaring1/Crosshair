from __future__ import annotations

import json
import multiprocessing as mp
import socket
import threading
from pathlib import Path

import pytest

import crosshair_core as core


def paths(tmp_path: Path) -> core.AppPaths:
    env = {
        "HOME": str(tmp_path / "home"),
        "XDG_CONFIG_HOME": str(tmp_path / "config"),
        "XDG_CACHE_HOME": str(tmp_path / "cache"),
        "XDG_STATE_HOME": str(tmp_path / "state"),
        "XDG_DATA_HOME": str(tmp_path / "data"),
        "XDG_RUNTIME_DIR": str(tmp_path / "runtime"),
    }
    for value in env.values():
        Path(value).mkdir(parents=True, exist_ok=True)
    return core.AppPaths.from_env(env)

@pytest.mark.parametrize("value", ["NaN", float("nan"), "inf", float("inf"), None, {}])
def test_non_finite_float_uses_default(value):
    assert core.as_float(value, 0.95, 0.05, 1.0) == 0.95


def test_monitor_cache_key_distinguishes_equal_sized_monitors():
    first = core.monitor_cache_key(597, 336, (0, 0, 2560, 1440), 100)
    second = core.monitor_cache_key(597, 336, (2560, 0, 2560, 1440), 200)
    assert first != second


def test_monitor_cache_key_is_stable_across_wrapper_recreation():
    first = core.monitor_cache_key(597, 336, (0, 0, 2560, 1440), 100)
    recreated = core.monitor_cache_key(597, 336, (0, 0, 2560, 1440), 999)
    assert first == recreated


def test_monitor_cache_key_changes_when_monitor_layout_changes():
    before = core.monitor_cache_key(597, 336, (0, 0, 2560, 1440), 100)
    moved = core.monitor_cache_key(597, 336, (2560, 0, 2560, 1440), 100)
    assert before != moved


def test_monitor_cache_key_fallback_does_not_conflate_equal_sizes():
    first = core.monitor_cache_key(597, 336, None, 100)
    second = core.monitor_cache_key(597, 336, None, 200)
    assert first != second


def test_dispatch_and_wait_acknowledges_completed_callback():
    calls = []
    success, error = core.dispatch_and_wait(
        lambda callback: callback(),
        lambda: calls.append("applied"),
    )
    assert success is True
    assert error is None
    assert calls == ["applied"]


def test_dispatch_and_wait_cancels_callback_that_never_started():
    queued = []
    calls = []
    success, error = core.dispatch_and_wait(
        lambda callback: queued.append(callback),
        lambda: calls.append("late"),
        timeout=0,
    )
    assert success is False
    assert "did not process" in error
    assert len(queued) == 1
    queued[0]()
    assert calls == []


def test_dispatch_and_wait_reports_callback_exception():
    def fail():
        raise ValueError("callback failed")

    success, error = core.dispatch_and_wait(lambda callback: callback(), fail)
    assert success is False
    assert error == "UI callback failed: ValueError"


def test_dispatch_and_wait_reports_callback_still_running():
    release = threading.Event()
    started = threading.Event()
    workers = []

    def schedule(callback):
        worker = threading.Thread(target=callback, daemon=True)
        workers.append(worker)
        worker.start()
        assert started.wait(1)

    def slow_callback():
        started.set()
        release.wait(1)

    success, error = core.dispatch_and_wait(
        schedule,
        slow_callback,
        timeout=0.01,
    )
    assert success is False
    assert error == "GTK main loop started the command but did not finish before timeout"
    release.set()
    workers[0].join(1)


def test_dispatch_and_wait_reports_scheduler_failure():
    def fail_schedule(_callback):
        raise RuntimeError("no scheduler")

    success, error = core.dispatch_and_wait(fail_schedule, lambda: None)
    assert success is False
    assert error == "could not schedule UI callback: RuntimeError"


def test_bad_nested_types_do_not_crash():
    cfg = core.sanitize_config({
        "size": "bad", "outline": "bad", "shadow": None, "color": "wrong"
    })
    assert cfg["size"] == 5
    assert cfg["color"] == "#a000ff"
    assert isinstance(cfg["outline"], dict)
    assert isinstance(cfg["shadow"], dict)

def test_unknown_patch_rejected():
    with pytest.raises(core.ConfigError):
        core.apply_patch(core.default_config(), {"surprise": True})

def test_patch_range_rejected():
    with pytest.raises(core.ConfigError):
        core.apply_patch(core.default_config(), {"opacity": 99})

def test_integer_patch_rejects_float():
    with pytest.raises(core.ConfigError):
        core.apply_patch(core.default_config(), {"size": 1.5})

def test_atomic_write_and_mode(tmp_path):
    target = tmp_path / "config.json"
    core.atomic_write_text(target, '{"ok":true}\n')
    assert json.loads(target.read_text()) == {"ok": True}
    assert target.stat().st_mode & 0o777 == 0o600
    assert list(tmp_path.glob(".config.json.*")) == []

def test_symlink_destination_rejected(tmp_path):
    real = tmp_path / "real"
    real.write_text("keep")
    link = tmp_path / "link"
    link.symlink_to(real)
    with pytest.raises(core.CoreError):
        core.atomic_write_text(link, "replace")
    assert real.read_text() == "keep"

def test_corrupt_config_preserved(tmp_path):
    app = paths(tmp_path)
    core.ensure_directory(app.config_dir)
    app.config_file.write_text('{"enabled":true,')
    result = core.load_config(app.config_file)
    assert result.recovered is True
    assert result.corrupt_copy and result.corrupt_copy.read_text() == '{"enabled":true,'
    assert result.config == core.default_config()

def _lock_worker(lock_path: str, ready, release, result):
    lock = core.DaemonLock.acquire(Path(lock_path))
    result.put(lock is not None)
    ready.set()
    if lock:
        release.wait(5)
        lock.close()

def test_only_one_lock_owner(tmp_path):
    app = paths(tmp_path)
    ready1, ready2, release = mp.Event(), mp.Event(), mp.Event()
    result = mp.Queue()
    first = mp.Process(
        target=_lock_worker,
        args=(str(app.lock_file), ready1, release, result),
    )
    first.start()
    assert ready1.wait(5)
    assert result.get(timeout=5) is True
    second = mp.Process(
        target=_lock_worker,
        args=(str(app.lock_file), ready2, release, result),
    )
    second.start()
    assert ready2.wait(5)
    assert result.get(timeout=5) is False
    release.set()
    first.join(5)
    second.join(5)
    assert first.exitcode == 0
    assert second.exitcode == 0

def test_message_limit():
    with pytest.raises(core.ProtocolError):
        core.encode_line({"cmd": "set", "payload": "x" * core.MAX_IPC_BYTES})

def test_ipc_round_trip(tmp_path):
    app = paths(tmp_path)
    log = core.setup_logging(app)

    def handler(request):
        if request["cmd"] == "status":
            return {"ok": True, "version": core.APP_VERSION}
        return {"ok": False, "message": "unknown"}

    server = core.IpcServer(app, handler, log)
    server.start()
    try:
        response = core.ipc_call(app, {"cmd": "status"})
        assert response["ok"] is True
        assert response["version"] == core.APP_VERSION
        response = core.ipc_call(app, {"cmd": "bad"})
        assert response["ok"] is False
        assert app.runtime_dir.stat().st_mode & 0o777 == 0o700
        assert app.socket_file.stat().st_mode & 0o777 == 0o600
    finally:
        server.stop()
    assert not app.socket_file.exists()

def test_bad_json_does_not_kill_server(tmp_path):
    app = paths(tmp_path)
    log = core.setup_logging(app)
    server = core.IpcServer(app, lambda _r: {"ok": True}, log)
    server.start()
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.connect(str(app.socket_file))
            client.sendall(b"not-json\n")
            response = json.loads(core.recv_line(client))
            assert response["ok"] is False
        assert core.ipc_call(app, {"cmd": "status"})["ok"] is True
    finally:
        server.stop()

def test_relative_xdg_paths_ignored(tmp_path):
    app = core.AppPaths.from_env({"HOME": str(tmp_path), "XDG_RUNTIME_DIR": "relative"})
    assert app.runtime_fallback is True
    assert app.runtime_dir.is_absolute()
