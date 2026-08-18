#!/usr/bin/env python3
"""Hardened, GTK-free core for Crosshair on Linux/Nobara.

No import-time writes, sockets, display access, migration, or process signals.
"""
from __future__ import annotations

import contextlib
import fcntl
import json
import logging
import math
import os
import re
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from logging.handlers import RotatingFileHandler
from pathlib import Path
from typing import IO, Any

APP_ID = "crosshair"
APP_NAME = "Crosshair"
APP_VERSION = "3.2.0-candidate"
MAX_IPC_BYTES = 64 * 1024
MAX_STRING_LENGTH = 256
SOCKET_PATH_LIMIT = 100

JsonObject = dict[str, Any]
IpcHandler = Callable[[JsonObject], Mapping[str, Any]]

class CoreError(RuntimeError):
    pass

class ConfigError(CoreError):
    pass

class AlreadyRunning(CoreError):
    pass

class ProtocolError(CoreError):
    pass

class IpcUnavailable(CoreError):
    pass

@dataclass(frozen=True)
class AppPaths:
    home: Path
    config_dir: Path
    config_file: Path
    runtime_dir: Path
    socket_file: Path
    lock_file: Path
    state_dir: Path
    log_file: Path
    data_dir: Path
    installed_source: Path
    bin_file: Path
    application_entry: Path
    autostart_entry: Path
    runtime_fallback: bool

    @classmethod
    def from_env(cls, env: Mapping[str, str] | None = None) -> AppPaths:
        env = os.environ if env is None else env
        fallback_home = Path.home()
        candidate_home = Path(env.get("HOME") or fallback_home).expanduser()
        home = candidate_home if candidate_home.is_absolute() else fallback_home

        def xdg(name: str, fallback: Path) -> Path:
            raw = env.get(name)
            candidate = Path(raw).expanduser() if raw else fallback
            return candidate if candidate.is_absolute() else fallback

        config_home = xdg("XDG_CONFIG_HOME", home / ".config")
        cache_home = xdg("XDG_CACHE_HOME", home / ".cache")
        state_home = xdg("XDG_STATE_HOME", home / ".local" / "state")
        data_home = xdg("XDG_DATA_HOME", home / ".local" / "share")
        runtime_raw = env.get("XDG_RUNTIME_DIR")
        runtime_candidate = Path(runtime_raw).expanduser() if runtime_raw else None
        runtime_valid = bool(runtime_candidate and runtime_candidate.is_absolute())
        runtime_home = runtime_candidate if runtime_valid else cache_home / "runtime"
        assert runtime_home is not None

        config_dir = config_home / APP_ID
        runtime_dir = runtime_home / APP_ID
        state_dir = state_home / APP_ID
        data_dir = data_home / APP_ID
        return cls(
            home=home,
            config_dir=config_dir,
            config_file=config_dir / "config.json",
            runtime_dir=runtime_dir,
            socket_file=runtime_dir / "ipc.sock",
            lock_file=runtime_dir / "daemon.lock",
            state_dir=state_dir,
            log_file=state_dir / "crosshair.log",
            data_dir=data_dir,
            installed_source=data_dir / "crosshair.py",
            bin_file=home / ".local" / "bin" / APP_ID,
            application_entry=data_home / "applications" / f"{APP_ID}.desktop",
            autostart_entry=config_home / "autostart" / f"{APP_ID}.desktop",
            runtime_fallback=not runtime_valid,
        )

def ensure_directory(path: Path, mode: int = 0o700) -> Path:
    path = Path(path)
    if path.is_symlink():
        raise CoreError(f"Refusing symlink directory: {path}")
    path.mkdir(parents=True, exist_ok=True, mode=mode)
    if not path.is_dir() or path.is_symlink():
        raise CoreError(f"Not a safe directory: {path}")
    path.chmod(mode)
    return path

def ensure_runtime(paths: AppPaths) -> None:
    ensure_directory(paths.runtime_dir, 0o700)
    length = len(os.fsencode(str(paths.socket_file)))
    if length >= SOCKET_PATH_LIMIT:
        raise CoreError(f"AF_UNIX path too long ({length} bytes): {paths.socket_file}")

def _fsync_directory(path: Path) -> None:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    try:
        fd = os.open(path, flags)
    except OSError:
        return
    try:
        os.fsync(fd)
    finally:
        os.close(fd)

def atomic_write_bytes(path: Path, data: bytes, *, mode: int = 0o600) -> None:
    path = Path(path)
    if path.parent.is_symlink():
        raise CoreError(f"Refusing symlink parent: {path.parent}")
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink():
        raise CoreError(f"Refusing symlink destination: {path}")

    fd, temp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temp = Path(temp_name)
    fd_open = True
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as handle:
            fd_open = False
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        if path.is_symlink():
            raise CoreError(f"Destination became a symlink: {path}")
        os.replace(temp, path)
        path.chmod(mode)
        _fsync_directory(path.parent)
    finally:
        if fd_open:
            with contextlib.suppress(OSError):
                os.close(fd)
        with contextlib.suppress(OSError):
            temp.unlink(missing_ok=True)

def atomic_write_text(path: Path, text: str, *, mode: int = 0o600) -> None:
    atomic_write_bytes(path, text.encode("utf-8"), mode=mode)

VALID_SHAPES = {
    "dot", "cross", "x", "plus", "ring", "circle",
    "cross_dot", "x_dot", "dot_ring",
}
VALID_MONITOR_MODES = {"all", "primary", "active", "index", "name"}
HEX_COLOR = re.compile(r"^#[0-9a-fA-F]{6}$")
ALLOWED_TOP = {
    "enabled", "shape", "opacity", "size", "thickness", "gap", "color",
    "outline", "shadow", "monitor_mode", "monitor_index", "monitor_name",
    "monitor_name_ordinal", "preview_bg", "auto_save",
}
ALLOWED_OUTLINE = {"enabled", "color", "opacity", "thickness"}
ALLOWED_SHADOW = {"enabled", "color", "opacity", "offset_x", "offset_y"}

def default_config() -> JsonObject:
    return {
        "enabled": True,
        "shape": "dot",
        "opacity": 0.95,
        "size": 5,
        "thickness": 2,
        "gap": 3,
        "color": "#a000ff",
        "outline": {
            "enabled": True,
            "color": "#000000",
            "opacity": 0.80,
            "thickness": 1,
        },
        "shadow": {
            "enabled": False,
            "color": "#000000",
            "opacity": 0.35,
            "offset_x": 1,
            "offset_y": 1,
        },
        "monitor_mode": "all",
        "monitor_index": 0,
        "monitor_name": "",
        "monitor_name_ordinal": 0,
        "preview_bg": "#222222",
        "auto_save": True,
    }

def as_bool(value: Any, default: bool) -> bool:
    if type(value) is bool:
        return value
    if isinstance(value, (int, float)) and not isinstance(value, bool) and value in (0, 1):
        return bool(value)
    if isinstance(value, str):
        lowered = value.strip().lower()
        if lowered in {"1", "true", "yes", "on"}:
            return True
        if lowered in {"0", "false", "no", "off"}:
            return False
    return default

def as_int(value: Any, default: int, lo: int, hi: int) -> int:
    if isinstance(value, bool):
        value = default
    try:
        parsed = int(value)
    except (TypeError, ValueError, OverflowError):
        parsed = default
    return max(lo, min(hi, parsed))

def as_float(value: Any, default: float, lo: float, hi: float) -> float:
    if isinstance(value, bool):
        value = default
    try:
        parsed = float(value)
    except (TypeError, ValueError, OverflowError):
        parsed = default
    if not math.isfinite(parsed):
        parsed = default
    return max(lo, min(hi, parsed))

def as_color(value: Any, default: str) -> str:
    candidate = str(value).strip()
    if not candidate.startswith("#"):
        candidate = "#" + candidate
    return candidate.lower() if HEX_COLOR.fullmatch(candidate) else default

def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}

def sanitize_config(raw: Any, *, strict_unknown: bool = False) -> JsonObject:
    if not isinstance(raw, Mapping):
        if strict_unknown:
            raise ConfigError("Configuration must be a JSON object")
        raw = {}
    unknown = set(raw) - ALLOWED_TOP
    if strict_unknown and unknown:
        raise ConfigError(f"Unknown config keys: {sorted(unknown)}")

    outline = _mapping(raw.get("outline"))
    shadow = _mapping(raw.get("shadow"))
    if strict_unknown:
        bad_outline = set(outline) - ALLOWED_OUTLINE
        bad_shadow = set(shadow) - ALLOWED_SHADOW
        if bad_outline:
            raise ConfigError(f"Unknown outline keys: {sorted(bad_outline)}")
        if bad_shadow:
            raise ConfigError(f"Unknown shadow keys: {sorted(bad_shadow)}")

    shape = str(raw.get("shape", "dot")).strip().lower()
    if shape not in VALID_SHAPES:
        shape = "dot"
    if shape == "circle":
        shape = "ring"
    monitor_mode = str(raw.get("monitor_mode", "all")).strip().lower()
    if monitor_mode not in VALID_MONITOR_MODES:
        monitor_mode = "all"
    monitor_name_raw = raw.get("monitor_name", "")
    monitor_name = monitor_name_raw if isinstance(monitor_name_raw, str) else ""

    return {
        "enabled": as_bool(raw.get("enabled"), True),
        "shape": shape,
        "opacity": as_float(raw.get("opacity"), 0.95, 0.05, 1.0),
        "size": as_int(raw.get("size"), 5, 1, 200),
        "thickness": as_int(raw.get("thickness"), 2, 1, 100),
        "gap": as_int(raw.get("gap"), 3, 0, 200),
        "color": as_color(raw.get("color"), "#a000ff"),
        "outline": {
            "enabled": as_bool(outline.get("enabled"), True),
            "color": as_color(outline.get("color"), "#000000"),
            "opacity": as_float(outline.get("opacity"), 0.80, 0.0, 1.0),
            "thickness": as_int(outline.get("thickness"), 1, 0, 100),
        },
        "shadow": {
            "enabled": as_bool(shadow.get("enabled"), False),
            "color": as_color(shadow.get("color"), "#000000"),
            "opacity": as_float(shadow.get("opacity"), 0.35, 0.0, 1.0),
            "offset_x": as_int(shadow.get("offset_x"), 1, -200, 200),
            "offset_y": as_int(shadow.get("offset_y"), 1, -200, 200),
        },
        "monitor_mode": monitor_mode,
        "monitor_index": as_int(raw.get("monitor_index"), 0, 0, 255),
        "monitor_name": monitor_name[:MAX_STRING_LENGTH],
        "monitor_name_ordinal": as_int(raw.get("monitor_name_ordinal"), 0, 0, 255),
        "preview_bg": as_color(raw.get("preview_bg"), "#222222"),
        "auto_save": as_bool(raw.get("auto_save"), True),
    }

@dataclass(frozen=True)
class ConfigLoadResult:
    config: JsonObject
    recovered: bool = False
    corrupt_copy: Path | None = None
    error: str | None = None

def load_config(path: Path, log: logging.Logger | None = None) -> ConfigLoadResult:
    path = Path(path)
    if not path.exists():
        return ConfigLoadResult(default_config())
    if path.is_symlink():
        raise ConfigError(f"Refusing symlink config: {path}")
    try:
        raw_bytes = path.read_bytes()
        parsed = json.loads(raw_bytes.decode("utf-8"))
        if isinstance(parsed, Mapping):
            unknown = set(parsed) - ALLOWED_TOP
            if unknown and log:
                log.warning("Ignoring unknown config keys: %s", sorted(unknown))
        return ConfigLoadResult(sanitize_config(parsed))
    except (OSError, UnicodeError, json.JSONDecodeError, ConfigError, ValueError) as exc:
        stamp = time.strftime("%Y%m%dT%H%M%S", time.localtime())
        backup = path.with_name(f"{path.name}.corrupt-{stamp}-{os.getpid()}")
        try:
            atomic_write_bytes(backup, path.read_bytes(), mode=0o600)
        except OSError as preserve_error:
            raise ConfigError(
                f"Invalid config could not be preserved: {preserve_error}"
            ) from exc
        message = f"Invalid config preserved at {backup}: {exc}"
        if log:
            log.error(message)
        return ConfigLoadResult(default_config(), True, backup, message)

def save_config(config: Mapping[str, Any], path: Path) -> JsonObject:
    clean = sanitize_config(config, strict_unknown=True)
    ensure_directory(Path(path).parent, 0o700)
    atomic_write_text(path, json.dumps(clean, indent=2, sort_keys=True) + "\n")
    return clean

def file_fingerprint(path: Path) -> tuple[int, int, int] | None:
    try:
        stat = Path(path).stat()
    except OSError:
        return None
    return (stat.st_ino, stat.st_size, stat.st_mtime_ns)

def setup_logging(paths: AppPaths, *, verbose: bool = False) -> logging.Logger:
    ensure_directory(paths.state_dir, 0o700)
    if paths.log_file.is_symlink():
        raise CoreError(f"Refusing symlink log: {paths.log_file}")
    logger = logging.getLogger(APP_ID)
    for old_handler in logger.handlers[:]:
        logger.removeHandler(old_handler)
        old_handler.close()
    logger.propagate = False
    logger.setLevel(logging.DEBUG if verbose else logging.INFO)
    formatter = logging.Formatter(
        "%(asctime)s %(levelname)s %(threadName)s %(name)s: %(message)s"
    )
    file_handler = RotatingFileHandler(
        paths.log_file, maxBytes=1_000_000, backupCount=3, encoding="utf-8"
    )
    paths.log_file.chmod(0o600)
    file_handler.setFormatter(formatter)
    logger.addHandler(file_handler)
    if verbose:
        stream = logging.StreamHandler()
        stream.setFormatter(formatter)
        logger.addHandler(stream)
    return logger

@dataclass
class DaemonLock:
    path: Path
    handle: IO[str]

    @classmethod
    def acquire(cls, path: Path) -> DaemonLock | None:
        path = Path(path)
        ensure_directory(path.parent, 0o700)
        if path.is_symlink():
            raise CoreError(f"Refusing symlink lock: {path}")
        flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_CLOEXEC", 0)
        flags |= getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(path, flags, 0o600)
        handle = os.fdopen(fd, "r+", encoding="utf-8")
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            handle.close()
            return None
        except Exception:
            handle.close()
            raise
        os.fchmod(handle.fileno(), 0o600)
        handle.seek(0)
        handle.truncate()
        handle.write(f"{os.getpid()}\n")
        handle.flush()
        os.fsync(handle.fileno())
        return cls(path, handle)

    def close(self) -> None:
        if self.handle.closed:
            return
        try:
            fcntl.flock(self.handle.fileno(), fcntl.LOCK_UN)
        finally:
            self.handle.close()

def encode_line(payload: Mapping[str, Any]) -> bytes:
    data = (json.dumps(dict(payload), separators=(",", ":")) + "\n").encode("utf-8")
    if len(data) > MAX_IPC_BYTES:
        raise ProtocolError("IPC message is too large")
    return data

def recv_line(conn: socket.socket) -> bytes:
    data = bytearray()
    while True:
        remaining = MAX_IPC_BYTES + 1 - len(data)
        if remaining <= 0:
            raise ProtocolError("IPC message is too large")
        chunk = conn.recv(min(4096, remaining))
        if not chunk:
            raise ProtocolError("Connection closed before newline")
        data.extend(chunk)
        if len(data) > MAX_IPC_BYTES:
            raise ProtocolError("IPC message is too large")
        newline = data.find(b"\n")
        if newline >= 0:
            return bytes(data[:newline])

def decode_object(raw: bytes) -> JsonObject:
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ProtocolError(f"Invalid JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise ProtocolError("IPC message must be an object")
    return value

def peer_uid(conn: socket.socket) -> int | None:
    if not hasattr(socket, "SO_PEERCRED"):
        return None
    size = struct.calcsize("3i")
    raw = conn.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, size)
    _pid, uid, _gid = struct.unpack("3i", raw)
    return uid

class IpcServer:
    def __init__(self, paths: AppPaths, handler: IpcHandler, log: logging.Logger):
        self.paths = paths
        self.handler = handler
        self.log = log
        self.lock: DaemonLock | None = None
        self.sock: socket.socket | None = None
        self.socket_inode: int | None = None
        self.stop_event = threading.Event()
        self.thread: threading.Thread | None = None

    def start(self) -> None:
        self.stop_event.clear()
        ensure_runtime(self.paths)
        self.lock = DaemonLock.acquire(self.paths.lock_file)
        if self.lock is None:
            raise AlreadyRunning("Crosshair daemon is already running")
        try:
            if self.paths.socket_file.is_symlink():
                raise CoreError(f"Refusing symlink socket: {self.paths.socket_file}")
            self.paths.socket_file.unlink(missing_ok=True)
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            old_umask = os.umask(0o077)
            try:
                self.sock.bind(str(self.paths.socket_file))
            finally:
                os.umask(old_umask)
            self.socket_inode = self.paths.socket_file.lstat().st_ino
            self.paths.socket_file.chmod(0o600)
            self.sock.listen(16)
            self.sock.settimeout(0.5)
            self.thread = threading.Thread(
                target=self._serve, name="crosshair-ipc", daemon=True
            )
            self.thread.start()
        except Exception:
            self._cleanup_socket()
            if self.lock:
                self.lock.close()
                self.lock = None
            raise

    def _serve(self) -> None:
        while not self.stop_event.is_set():
            if self.sock is None:
                return
            try:
                conn, _ = self.sock.accept()
            except TimeoutError:
                continue
            except OSError:
                if not self.stop_event.is_set():
                    self.log.exception("IPC accept failed")
                return
            with conn:
                conn.settimeout(1.0)
                try:
                    uid = peer_uid(conn)
                    if uid is not None and uid != os.getuid():
                        raise ProtocolError(f"Rejected peer uid {uid}")
                    request = decode_object(recv_line(conn))
                    if not isinstance(request.get("cmd"), str):
                        raise ProtocolError("Missing string command")
                    response = dict(self.handler(request))
                    response.setdefault("ok", True)
                except ProtocolError as exc:
                    response = {"ok": False, "message": str(exc)}
                except Exception as exc:
                    self.log.exception("IPC handler failed")
                    response = {
                        "ok": False,
                        "message": f"internal error: {type(exc).__name__}",
                    }
                try:
                    conn.sendall(encode_line(response))
                except (OSError, ProtocolError):
                    self.log.exception("IPC response failed")

    def _cleanup_socket(self) -> None:
        sock, self.sock = self.sock, None
        expected_inode, self.socket_inode = self.socket_inode, None
        if sock:
            with contextlib.suppress(OSError):
                sock.close()
        try:
            inode = self.paths.socket_file.lstat().st_ino
        except OSError:
            return
        if expected_inode is not None and inode == expected_inode:
            self.paths.socket_file.unlink(missing_ok=True)

    def stop(self) -> None:
        self.stop_event.set()
        self._cleanup_socket()
        if self.thread and self.thread is not threading.current_thread():
            self.thread.join(timeout=2.0)
        self.thread = None
        if self.lock:
            self.lock.close()
            self.lock = None

def ipc_call(
    paths: AppPaths, request: Mapping[str, Any], *, timeout: float = 0.75
) -> JsonObject:
    payload = encode_line(request)
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(timeout)
            client.connect(str(paths.socket_file))
            client.sendall(payload)
            response = decode_object(recv_line(client))
    except (TimeoutError, FileNotFoundError, ConnectionRefusedError, OSError) as exc:
        raise IpcUnavailable(str(exc)) from exc
    if type(response.get("ok")) is not bool:
        raise ProtocolError("Response is missing boolean ok")
    return response

def _number(name: str, value: Any, lo: float, hi: float) -> None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ConfigError(f"{name} must be numeric")
    numeric = float(value)
    if not math.isfinite(numeric) or not lo <= numeric <= hi:
        raise ConfigError(f"{name} must be between {lo} and {hi}")

def _integer(name: str, value: Any, lo: int, hi: int) -> None:
    if type(value) is not int or not lo <= value <= hi:
        raise ConfigError(f"{name} must be an integer between {lo} and {hi}")

def validate_patch(payload: Any) -> JsonObject:
    if not isinstance(payload, Mapping):
        raise ConfigError("Config patch must be an object")
    unknown = set(payload) - ALLOWED_TOP
    if unknown:
        raise ConfigError(f"Unknown patch keys: {sorted(unknown)}")
    result = dict(payload)
    float_ranges = {"opacity": (0.05, 1.0)}
    integer_ranges = {
        "size": (1, 200),
        "thickness": (1, 100),
        "gap": (0, 200),
        "monitor_index": (0, 255),
        "monitor_name_ordinal": (0, 255),
    }
    for key, value in result.items():
        if key in {"outline", "shadow"}:
            if not isinstance(value, Mapping):
                raise ConfigError(f"{key} must be an object")
            allowed = ALLOWED_OUTLINE if key == "outline" else ALLOWED_SHADOW
            bad = set(value) - allowed
            if bad:
                raise ConfigError(f"Unknown {key} keys: {sorted(bad)}")
            for nested_key, nested_value in value.items():
                full = f"{key}.{nested_key}"
                if nested_key == "enabled" and type(nested_value) is not bool:
                    raise ConfigError(f"{full} must be boolean")
                if nested_key == "color" and not HEX_COLOR.fullmatch(str(nested_value)):
                    raise ConfigError(f"{full} must be #RRGGBB")
                if nested_key == "opacity":
                    _number(full, nested_value, 0.0, 1.0)
                if nested_key == "thickness":
                    _integer(full, nested_value, 0, 100)
                if nested_key in {"offset_x", "offset_y"}:
                    _integer(full, nested_value, -200, 200)
            continue
        if key in {"enabled", "auto_save"} and type(value) is not bool:
            raise ConfigError(f"{key} must be boolean")
        if key in float_ranges:
            _number(key, value, *float_ranges[key])
        if key in integer_ranges:
            _integer(key, value, *integer_ranges[key])
        if key in {"color", "preview_bg"} and not HEX_COLOR.fullmatch(str(value)):
            raise ConfigError(f"{key} must be #RRGGBB")
        if key == "shape" and str(value).lower() not in VALID_SHAPES:
            raise ConfigError(f"Unsupported shape: {value}")
        if key == "monitor_mode" and str(value).lower() not in VALID_MONITOR_MODES:
            raise ConfigError(f"Unsupported monitor mode: {value}")
        if key == "monitor_name" and (
            not isinstance(value, str) or len(value) > MAX_STRING_LENGTH
        ):
            raise ConfigError("monitor_name is invalid or too long")
    return result

def deep_merge(base: Mapping[str, Any], patch: Mapping[str, Any]) -> JsonObject:
    out = dict(base)
    for key, value in patch.items():
        if isinstance(value, Mapping) and isinstance(out.get(key), Mapping):
            out[key] = deep_merge(out[key], value)
        else:
            out[key] = value
    return out

def apply_patch(config: Mapping[str, Any], payload: Any) -> JsonObject:
    return sanitize_config(
        deep_merge(config, validate_patch(payload)), strict_unknown=True
    )

def schedule_restart(script: Path, argv: list[str], log_file: Path) -> int:
    """Return a guard FD; close it after cleanup to launch the replacement."""
    log_file = Path(log_file)
    ensure_directory(log_file.parent, 0o700)
    if log_file.is_symlink():
        raise CoreError(f"Refusing symlink restart log: {log_file}")
    log_flags = os.O_WRONLY | os.O_CREAT | os.O_APPEND
    log_flags |= getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    log_fd = os.open(log_file, log_flags, 0o600)
    os.fchmod(log_fd, 0o600)
    read_fd, write_fd = os.pipe()
    os.set_inheritable(read_fd, True)
    helper = (
        "import os,subprocess,sys\n"
        "fd=int(sys.argv[1]); script=sys.argv[2]; args=sys.argv[3:]\n"
        "try:\n"
        "    while os.read(fd,4096): pass\n"
        "finally:\n"
        "    os.close(fd)\n"
        "subprocess.Popen([sys.executable,script,*args],"
        "stdin=subprocess.DEVNULL,start_new_session=True,close_fds=True)\n"
    )
    command = [
        sys.executable, "-c", helper, str(read_fd), str(Path(script).resolve()), *argv
    ]
    try:
        subprocess.Popen(
            command,
            pass_fds=(read_fd,),
            close_fds=True,
            stdin=subprocess.DEVNULL,
            stdout=log_fd,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    except Exception:
        os.close(read_fd)
        os.close(write_fd)
        raise
    finally:
        os.close(log_fd)
    os.close(read_fd)
    return write_fd

def release_restart_guard(write_fd: int | None) -> None:
    if write_fd is not None:
        with contextlib.suppress(OSError):
            os.close(write_fd)
