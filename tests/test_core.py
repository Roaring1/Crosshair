from __future__ import annotations

import json
import multiprocessing as mp
import os
import socket
import threading
import time
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


def test_logging_refuses_log_symlink_swapped_after_precheck(tmp_path, monkeypatch):
    app = paths(tmp_path)
    core.ensure_directory(app.state_dir)
    target = tmp_path / "outside-log"
    target.write_text("preserve", encoding="utf-8")
    target.chmod(0o644)
    original_open = core.os.open

    def swap_before_open(path, flags, *args, **kwargs):
        if Path(path) == app.log_file and not app.log_file.exists():
            app.log_file.symlink_to(target)
        return original_open(path, flags, *args, **kwargs)

    monkeypatch.setattr(core.os, "open", swap_before_open)

    with pytest.raises(core.CoreError, match="Could not safely open log"):
        core.setup_logging(app)

    assert app.log_file.is_symlink()
    assert target.read_text(encoding="utf-8") == "preserve"
    assert target.stat().st_mode & 0o777 == 0o644


def test_logging_rejects_fifo_without_blocking(tmp_path):
    app = paths(tmp_path)
    core.ensure_directory(app.state_dir)
    os.mkfifo(app.log_file)

    with pytest.raises(core.CoreError, match="Could not safely open log"):
        core.setup_logging(app)


def test_atomic_write_does_not_chmod_a_swapped_symlink_target(tmp_path, monkeypatch):
    destination = tmp_path / "config.json"
    target = tmp_path / "external-target"
    target.write_text("preserve", encoding="utf-8")
    target.chmod(0o644)
    original_replace = core.os.replace

    def swap_destination_for_symlink(source, dest, **kwargs):
        original_replace(source, dest, **kwargs)
        os.unlink(dest, dir_fd=kwargs["dst_dir_fd"])
        os.symlink(target, dest, dir_fd=kwargs["dst_dir_fd"])

    monkeypatch.setattr(core.os, "replace", swap_destination_for_symlink)

    core.atomic_write_bytes(destination, b"new config", mode=0o600)

    assert destination.is_symlink()
    assert target.read_text(encoding="utf-8") == "preserve"
    assert target.stat().st_mode & 0o777 == 0o644


def test_atomic_write_refuses_parent_swapped_to_symlink(tmp_path, monkeypatch):
    directory = tmp_path / "private"
    target = tmp_path / "external-directory"
    target.mkdir()
    sentinel = target / "keep.txt"
    sentinel.write_text("preserve", encoding="utf-8")
    original_open = core.os.open
    swapped = False

    def swap_parent_after_open(path, flags, *args, **kwargs):
        nonlocal swapped
        fd = original_open(path, flags, *args, **kwargs)
        if Path(path) == directory and not swapped:
            directory.rmdir()
            directory.symlink_to(target, target_is_directory=True)
            swapped = True
        return fd

    monkeypatch.setattr(core.os, "open", swap_parent_after_open)

    with pytest.raises(core.CoreError, match="Atomic-write parent changed"):
        core.atomic_write_bytes(directory / "config.json", b"attacker-controlled")

    assert swapped is True
    assert directory.is_symlink()
    assert sentinel.read_text(encoding="utf-8") == "preserve"
    assert sorted(path.name for path in target.iterdir()) == ["keep.txt"]


def test_ensure_directory_does_not_chmod_a_swapped_symlink_target(tmp_path, monkeypatch):
    directory = tmp_path / "private"
    target = tmp_path / "external-directory"
    target.mkdir()
    target.chmod(0o755)
    original_open = core.os.open
    swapped = False

    def swap_after_open(path, flags, *args, **kwargs):
        nonlocal swapped
        fd = original_open(path, flags, *args, **kwargs)
        if Path(path) == directory and not swapped:
            directory.rmdir()
            directory.symlink_to(target, target_is_directory=True)
            swapped = True
        return fd

    monkeypatch.setattr(core.os, "open", swap_after_open)

    with pytest.raises(core.CoreError, match="Directory changed during validation"):
        core.ensure_directory(directory, 0o700)

    assert swapped is True
    assert directory.is_symlink()
    assert target.stat().st_mode & 0o777 == 0o755

def test_corrupt_config_preserved(tmp_path):
    app = paths(tmp_path)
    core.ensure_directory(app.config_dir)
    app.config_file.write_text('{"enabled":true,')
    result = core.load_config(app.config_file)
    assert result.recovered is True
    assert result.corrupt_copy and result.corrupt_copy.read_text() == '{"enabled":true,'
    assert result.config == core.default_config()


def test_oversized_config_is_not_read_or_backed_up(tmp_path):
    app = paths(tmp_path)
    core.ensure_directory(app.config_dir)
    with app.config_file.open("wb") as handle:
        handle.truncate(core.MAX_CONFIG_BYTES + 1)

    result = core.load_config(app.config_file)

    assert result.recovered is True
    assert result.corrupt_copy is None
    assert "safety limit" in result.error
    assert app.config_file.stat().st_size == core.MAX_CONFIG_BYTES + 1
    assert result.config == core.default_config()
    assert list(app.config_dir.glob("*.corrupt-*")) == []


@pytest.mark.parametrize("dangling", [False, True])
def test_load_config_refuses_symlink_without_reading_target(tmp_path, dangling):
    app = paths(tmp_path)
    core.ensure_directory(app.config_dir)
    target = tmp_path / "outside-config.json"
    if not dangling:
        target.write_text('{"enabled": false}', encoding="utf-8")
    app.config_file.symlink_to(target)

    with pytest.raises(core.ConfigError, match="symlink config"):
        core.load_config(app.config_file)

    if not dangling:
        assert target.read_text(encoding="utf-8") == '{"enabled": false}'


def test_load_config_rejects_non_regular_file(tmp_path):
    app = paths(tmp_path)
    core.ensure_directory(app.config_dir)
    app.config_file.mkdir()

    with pytest.raises(core.ConfigError, match="non-regular"):
        core.load_config(app.config_file)


def test_non_object_config_is_preserved_as_corrupt(tmp_path):
    app = paths(tmp_path)
    core.ensure_directory(app.config_dir)
    app.config_file.write_text('["unexpected"]', encoding="utf-8")

    result = core.load_config(app.config_file)

    assert result.recovered is True
    assert result.corrupt_copy is not None
    assert result.corrupt_copy.read_text(encoding="utf-8") == '["unexpected"]'
    assert result.config == core.default_config()


@pytest.mark.parametrize(
    "raw",
    [
        '{"enabled": true, "enabled": false}',
        '{"outline": {"enabled": true, "enabled": false}}',
        '{"opacity": NaN}',
        '{"opacity": Infinity}',
    ],
)
def test_config_rejects_ambiguous_or_nonstandard_json(tmp_path, raw):
    app = paths(tmp_path)
    core.ensure_directory(app.config_dir)
    app.config_file.write_text(raw, encoding="utf-8")

    result = core.load_config(app.config_file)

    assert result.recovered is True
    assert result.corrupt_copy is not None
    assert result.corrupt_copy.read_text(encoding="utf-8") == raw
    assert result.config == core.default_config()


def test_deeply_nested_config_is_recovered_without_crashing(tmp_path):
    app = paths(tmp_path)
    core.ensure_directory(app.config_dir)
    raw = "[" * 2000 + "0" + "]" * 2000
    app.config_file.write_text(raw, encoding="utf-8")

    result = core.load_config(app.config_file)

    assert result.recovered is True
    assert result.corrupt_copy is not None
    assert result.corrupt_copy.read_text(encoding="utf-8") == raw
    assert result.config == core.default_config()


def test_decode_object_rejects_duplicate_keys_and_nonstandard_constants():
    with pytest.raises(core.ProtocolError, match="duplicate JSON object key"):
        core.decode_object(b'{"cmd":"status","cmd":"quit"}')
    with pytest.raises(core.ProtocolError, match="non-standard JSON constant"):
        core.decode_object(b'{"cmd":"set","value":NaN}')


def test_encode_line_rejects_nonstandard_json_numbers():
    with pytest.raises(ValueError, match="Out of range float"):
        core.encode_line({"cmd": "set", "value": float("nan")})


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


def test_ipc_duplicate_keys_are_rejected_before_handler(tmp_path):
    app = paths(tmp_path)
    log = core.setup_logging(app)
    handled = []
    server = core.IpcServer(app, lambda request: handled.append(request) or {"ok": True}, log)
    server.start()
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.connect(str(app.socket_file))
            client.sendall(b'{"cmd":"status","cmd":"quit"}\n')
            response = json.loads(core.recv_line(client))
        assert response["ok"] is False
        assert "duplicate JSON object key" in response["message"]
        assert handled == []
        assert core.ipc_call(app, {"cmd": "status"}) == {"ok": True}
    finally:
        server.stop()

def test_relative_xdg_paths_ignored(tmp_path):
    app = core.AppPaths.from_env({"HOME": str(tmp_path), "XDG_RUNTIME_DIR": "relative"})
    assert app.runtime_fallback is True
    assert app.runtime_dir.is_absolute()


def test_restart_refuses_fifo_log_without_spawning_helper(tmp_path, monkeypatch):
    app = paths(tmp_path)
    core.ensure_directory(app.state_dir)
    os.mkfifo(app.log_file)
    spawned = []

    def unexpected_spawn(*args, **kwargs):
        spawned.append((args, kwargs))
        raise AssertionError("restart helper must not spawn with a special-file log")

    monkeypatch.setattr(core.subprocess, "Popen", unexpected_spawn)

    with pytest.raises(
        core.CoreError,
        match=r"Could not safely open restart log|non-regular restart log",
    ):
        core.schedule_restart(
            Path(__file__),
            ["--daemon", "--no-ui"],
            app.log_file,
        )

    assert spawned == []


@pytest.mark.parametrize("failure_point", ["pipe", "inheritable"])
def test_schedule_restart_closes_descriptors_when_setup_fails(
    tmp_path, monkeypatch, failure_point
):
    app = paths(tmp_path)
    core.ensure_directory(app.state_dir)
    opened_log_fds = []
    pipe_fds = []
    real_open = core.os.open
    real_pipe = core.os.pipe

    def capture_open(path, flags, *args, **kwargs):
        fd = real_open(path, flags, *args, **kwargs)
        if Path(path) == app.log_file:
            opened_log_fds.append(fd)
        return fd

    def fail_pipe():
        raise OSError("injected pipe setup failure")

    def capture_pipe():
        fds = real_pipe()
        pipe_fds.extend(fds)
        return fds

    monkeypatch.setattr(core.os, "open", capture_open)
    monkeypatch.setattr(
        core.subprocess,
        "Popen",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("helper must not spawn after descriptor setup failure")
        ),
    )
    if failure_point == "pipe":
        monkeypatch.setattr(core.os, "pipe", fail_pipe)
    else:
        monkeypatch.setattr(core.os, "pipe", capture_pipe)
        monkeypatch.setattr(
            core.os,
            "set_inheritable",
            lambda *_args, **_kwargs: (_ for _ in ()).throw(
                OSError("injected inheritable setup failure")
            ),
        )

    with pytest.raises(OSError):
        core.schedule_restart(
            Path(__file__),
            ["--daemon", "--no-ui"],
            app.log_file,
        )

    for fd in [*opened_log_fds, *pipe_fds]:
        with pytest.raises(OSError):
            os.fstat(fd)


def test_recv_line_rejects_oversized_message():
    server, client = socket.socketpair()
    try:
        client.sendall(b"x" * (core.MAX_IPC_BYTES + 1))
        with pytest.raises(core.ProtocolError, match="too large"):
            core.recv_line(server)
    finally:
        server.close()
        client.close()


def test_daemon_lock_refuses_symlink_without_touching_target(tmp_path):
    app = paths(tmp_path)
    core.ensure_directory(app.runtime_dir)
    target = tmp_path / "lock-target"
    target.write_text("preserve", encoding="utf-8")
    app.lock_file.symlink_to(target)

    with pytest.raises(core.CoreError, match="symlink lock"):
        core.DaemonLock.acquire(app.lock_file)

    assert target.read_text(encoding="utf-8") == "preserve"
    assert app.lock_file.is_symlink()


def test_daemon_lock_refuses_fifo_without_blocking(tmp_path):
    app = paths(tmp_path)
    core.ensure_runtime(app)
    os.mkfifo(app.lock_file)

    with pytest.raises(core.CoreError, match="non-regular lock"):
        core.DaemonLock.acquire(app.lock_file)


def test_ipc_server_refuses_symlink_socket_and_releases_lock(tmp_path):
    app = paths(tmp_path)
    core.ensure_runtime(app)
    target = tmp_path / "socket-target"
    target.write_text("preserve", encoding="utf-8")
    app.socket_file.symlink_to(target)
    server = core.IpcServer(app, lambda _request: {"ok": True}, core.setup_logging(app))

    with pytest.raises(core.CoreError, match="symlink socket"):
        server.start()

    assert target.read_text(encoding="utf-8") == "preserve"
    assert app.socket_file.is_symlink()
    # A failed start must not strand the single-instance lock.
    lock = core.DaemonLock.acquire(app.lock_file)
    assert lock is not None
    lock.close()


def test_ipc_server_refuses_to_delete_non_socket_path(tmp_path):
    app = paths(tmp_path)
    core.ensure_runtime(app)
    app.socket_file.write_text("preserve", encoding="utf-8")
    server = core.IpcServer(app, lambda _request: {"ok": True}, core.setup_logging(app))

    with pytest.raises(core.CoreError, match="non-socket IPC path"):
        server.start()

    assert app.socket_file.read_text(encoding="utf-8") == "preserve"
    lock = core.DaemonLock.acquire(app.lock_file)
    assert lock is not None
    lock.close()


def test_ipc_server_stop_wakes_blocked_accept_without_polling(tmp_path):
    app = paths(tmp_path)
    server = core.IpcServer(app, lambda _request: {"ok": True}, core.setup_logging(app))
    server.start()
    worker = server.thread
    assert worker is not None and worker.is_alive()

    started = time.monotonic()
    server.stop()

    assert time.monotonic() - started < 1.0
    assert not worker.is_alive()
    assert not app.socket_file.exists()


def test_ipc_shutdown_does_not_unlink_replacement_file(tmp_path):
    app = paths(tmp_path)
    server = core.IpcServer(app, lambda _request: {"ok": True}, core.setup_logging(app))
    server.start()

    app.socket_file.unlink()
    app.socket_file.write_text("replacement must survive", encoding="utf-8")
    server.stop()

    assert app.socket_file.read_text(encoding="utf-8") == "replacement must survive"


def test_ipc_rejects_unverifiable_peer_credentials_without_dispatch(
    tmp_path, monkeypatch
):
    app = paths(tmp_path)
    handled = []
    server = core.IpcServer(
        app,
        lambda request: handled.append(request) or {"ok": True},
        core.setup_logging(app),
    )
    responses = []

    class FakeConnection:
        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return None

        def settimeout(self, _timeout):
            return None

        def recv(self, _size):
            raise AssertionError("unauthenticated IPC payload must not be parsed")

        def sendall(self, payload):
            responses.append(core.decode_object(payload.rstrip(bytes([10]))))

    connection = FakeConnection()

    class FakeListener:
        def accept(self):
            return connection, None

    server.sock = FakeListener()

    def unverifiable_peer(_connection):
        server.stop_event.set()
        return None

    monkeypatch.setattr(core, "peer_uid", unverifiable_peer)

    server._serve()

    assert responses == [
        {
            "ok": False,
            "message": "Peer credential verification is unavailable",
        }
    ]
    assert handled == []


def test_ipc_rejects_different_peer_uid_and_remains_available(tmp_path, monkeypatch):
    app = paths(tmp_path)
    server = core.IpcServer(app, lambda _request: {"ok": True}, core.setup_logging(app))
    server.start()
    try:
        with monkeypatch.context() as patch:
            patch.setattr(core, "peer_uid", lambda _connection: -1)
            rejected = core.ipc_call(app, {"cmd": "status"})
        assert rejected == {"ok": False, "message": "Rejected peer uid -1"}

        assert core.ipc_call(app, {"cmd": "status"}) == {"ok": True}
    finally:
        server.stop()
