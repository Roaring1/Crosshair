from __future__ import annotations

import io
import shlex
import subprocess
import sys
from pathlib import Path

import pytest

import crosshair
import crosshair_core as core


def app_paths(tmp_path):
    home = tmp_path / "home"
    return core.AppPaths.from_env(
        {
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(tmp_path / "config"),
            "XDG_CACHE_HOME": str(tmp_path / "cache"),
            "XDG_STATE_HOME": str(tmp_path / "state"),
            "XDG_DATA_HOME": str(tmp_path / "data"),
            "XDG_RUNTIME_DIR": str(tmp_path / "runtime"),
        }
    )


def test_monitor_selection_values_preserve_duplicate_ordinal():
    values = crosshair._monitor_selection_values(
        "name", 0, "Sceptre Tech Inc Sceptre Y27", 1
    )
    assert values == {
        "monitor_mode": "name",
        "monitor_index": 0,
        "monitor_name": "Sceptre Tech Inc Sceptre Y27",
        "monitor_name_ordinal": 1,
    }


def test_backend_classifier_prefers_actual_gdk_type():
    assert crosshair._backend_from_type_name("GdkWaylandDisplay", "x11") == "wayland"
    assert crosshair._backend_from_type_name("GdkX11Display", "wayland") == "x11"


def test_backend_classifier_uses_fallback_only_when_type_is_unknown():
    assert crosshair._backend_from_type_name("GdkDisplay", "wayland") == "wayland"
    assert crosshair._backend_from_type_name("", "") == "unknown"


def test_x_canvas_bounds_include_diagonal_endpoints():
    config = core.default_config()
    config.update({"shape": "x", "size": 200, "gap": 200, "thickness": 1})
    config["outline"].update({"enabled": False, "thickness": 0})
    config["shadow"]["enabled"] = False

    canvas = crosshair._canvas_size_px(core.sanitize_config(config))
    radial_endpoint = 2 ** 0.5 * (200 + 200)
    assert canvas / 2 >= radial_endpoint + 1


def test_x_dot_canvas_bounds_include_diagonal_endpoints():
    config = core.default_config()
    config.update({"shape": "x_dot", "size": 100, "gap": 80, "thickness": 2})
    config["outline"].update({"enabled": False, "thickness": 0})
    config["shadow"].update({"enabled": True, "offset_x": 30, "offset_y": -20})

    canvas = crosshair._canvas_size_px(core.sanitize_config(config))
    radial_endpoint = 2 ** 0.5 * (100 + 80)
    assert canvas / 2 >= radial_endpoint + 30 + 2


def test_preview_scale_leaves_normal_crosshair_at_native_size():
    config = core.sanitize_config(core.default_config())
    assert crosshair._preview_scale(config, 240, 240) == 1.0


def test_preview_scale_fits_large_crosshair_inside_available_area():
    config = core.default_config()
    config.update({"shape": "x", "size": 200, "gap": 200, "thickness": 100})
    config["outline"].update({"enabled": True, "thickness": 100})
    config["shadow"].update({"enabled": True, "offset_x": 200, "offset_y": -150})
    config = core.sanitize_config(config)

    scale = crosshair._preview_scale(config, 240, 240)
    extent = (crosshair._canvas_size_px(config) - 8) / 2
    assert 0 < scale < 1
    assert extent * scale <= 112


def test_preview_scale_handles_zero_sized_allocation():
    assert crosshair._preview_scale(core.default_config(), 0, 0) == 1.0


def test_safe_main_handles_broken_stdout(monkeypatch):
    stream = io.StringIO()
    monkeypatch.setattr(sys, "stdout", stream)

    def broken_main():
        raise BrokenPipeError("consumer closed the pipe")

    monkeypatch.setattr(crosshair, "main", broken_main)

    assert crosshair._safe_main() == 0
    assert stream.closed


def test_uninstall_removes_desktop_shortcut(tmp_path, monkeypatch):
    paths = app_paths(tmp_path)
    desktop_dir = tmp_path / "Desktop"
    desktop_dir.mkdir()
    monkeypatch.setattr(crosshair, "_desktop_dir", lambda: desktop_dir)
    monkeypatch.setattr(crosshair, "_remove_legacy_systemd_unit", lambda: [])

    artifacts = [
        paths.autostart_entry,
        paths.application_entry,
        paths.bin_file,
        paths.installed_source,
        paths.installed_source.with_name("crosshair_core.py"),
        crosshair.desktop_shortcut_path(),
    ]
    for artifact in artifacts:
        artifact.parent.mkdir(parents=True, exist_ok=True)
        artifact.write_text("desktop test artifact", encoding="utf-8")

    removed = crosshair.uninstall_app(paths)

    assert {str(path) for path in artifacts} <= set(removed)
    assert all(not path.exists() for path in artifacts)
    assert paths.config_dir.exists() is False


def test_install_self_refuses_symlink_source(tmp_path, monkeypatch):
    real_source = tmp_path / "real_crosshair.py"
    real_source.write_text("# real source\n", encoding="utf-8")
    linked_source = tmp_path / "crosshair-link.py"
    linked_source.symlink_to(real_source)
    monkeypatch.setattr(crosshair, "__file__", str(linked_source))

    paths = app_paths(tmp_path)
    with pytest.raises(core.CoreError, match="symlink source"):
        crosshair.install_self(paths)

    assert not paths.installed_source.exists()
    assert not paths.bin_file.exists()


def test_desktop_entry_exec_escapes_reserved_characters_and_field_codes():
    path = Path('/tmp/Crosshair 100% $HOME `command` "quoted"\\backslash')
    command = crosshair._desktop_entry_exec(path)

    assert command.startswith('"') and command.endswith('"')
    assert "100%%" in command
    assert r"\\$HOME" in command
    assert r"\\`command\\`" in command
    assert r'\\\"quoted\\\"' in command
    assert r"\\\\backslash" in command


@pytest.mark.parametrize("path", [Path("/tmp/Crosshair=unsafe"), Path("/tmp/Crosshair\nExec=Bad")])
def test_desktop_entry_exec_rejects_unsafe_executable_paths(path):
    with pytest.raises(core.CoreError, match=r"cannot contain|control characters"):
        crosshair._desktop_entry_exec(path)


def test_desktop_entry_string_escapes_literal_backslashes():
    assert crosshair._desktop_entry_string(Path(r"/tmp/Crosshair\\folder")) == (
        r"/tmp/Crosshair\\\\folder"
    )


def test_trusted_executable_accepts_root_owned_system_binary():
    executable = crosshair._trusted_executable("python3")
    if executable is None:
        pytest.skip("no trusted system Python binary in configured system directories")
    path = Path(executable)
    info = path.stat()
    assert path.is_absolute()
    assert path.is_file()
    assert info.st_uid == 0


def test_trusted_executable_rejects_user_owned_path_lookalike(tmp_path, monkeypatch):
    lookalike = tmp_path / "xdg-open"
    lookalike.write_text("#!/bin/sh\\nexit 0\\n", encoding="utf-8")
    lookalike.chmod(0o755)
    monkeypatch.setattr(crosshair, "_TRUSTED_SYSTEM_BIN_DIRS", (tmp_path,))

    assert crosshair._trusted_executable("xdg-open") is None
    assert crosshair._trusted_executable("../xdg-open") is None


@pytest.mark.parametrize("failure", ["disable", "daemon-reload"])
def test_legacy_systemd_unit_is_preserved_when_cleanup_fails(
    tmp_path, monkeypatch, failure
):
    home = tmp_path / "home"
    unit = home / ".config" / "systemd" / "user" / "crosshair.service"
    unit.parent.mkdir(parents=True)
    unit.write_text("[Service]\nExecStart=/old/crosshair\n", encoding="utf-8")

    systemctl = tmp_path / "systemctl"
    systemctl.write_text(
        "#!/bin/sh\n"
        f'if [ "$2" = "{failure}" ]; then exit 1; fi\n'
        "exit 0\n",
        encoding="utf-8",
    )
    systemctl.chmod(0o755)
    monkeypatch.setattr(crosshair.Path, "home", classmethod(lambda _cls: home))
    monkeypatch.setattr(
        crosshair,
        "_trusted_executable",
        lambda name: str(systemctl) if name == "systemctl" else None,
    )

    assert crosshair._remove_legacy_systemd_unit() == []
    assert unit.read_text(encoding="utf-8") == "[Service]\nExecStart=/old/crosshair\n"


def test_install_rejects_unsupported_exec_path_before_writing(tmp_path):
    home = tmp_path / "home=unsupported"
    paths = core.AppPaths.from_env({
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(tmp_path / "config"),
        "XDG_CACHE_HOME": str(tmp_path / "cache"),
        "XDG_STATE_HOME": str(tmp_path / "state"),
        "XDG_RUNTIME_DIR": str(tmp_path / "runtime"),
    })

    with pytest.raises(core.CoreError, match="cannot contain"):
        crosshair.install_self(paths)

    assert not paths.installed_source.exists()
    assert not paths.installed_source.with_name("crosshair_core.py").exists()
    assert not paths.bin_file.exists()


def test_installed_launcher_treats_paths_as_data_not_shell_syntax(tmp_path, monkeypatch):
    marker_name = "shell-injection-fired"
    marker = tmp_path / marker_name
    home = tmp_path / f"home $(touch {marker_name}) `touch {marker_name}` 'quoted'"
    home.mkdir()
    paths = core.AppPaths.from_env({
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(tmp_path / "config"),
        "XDG_CACHE_HOME": str(tmp_path / "cache"),
        "XDG_STATE_HOME": str(tmp_path / "state"),
        "XDG_RUNTIME_DIR": str(tmp_path / "runtime"),
    })

    captured = tmp_path / "captured-arguments"
    fake_interpreter = tmp_path / "fake interpreter 'with spaces'"
    fake_interpreter.write_text(
        "#!/bin/sh\n"
        f"printf '%s\\n' \"$@\" > {shlex.quote(str(captured))}\n",
        encoding="utf-8",
    )
    fake_interpreter.chmod(0o755)
    monkeypatch.setattr(crosshair.sys, "executable", str(fake_interpreter))
    monkeypatch.setattr(crosshair, "_validate_desktop_file", lambda _path: None)

    crosshair.install_self(paths)
    injected_argument = f"$(touch {marker_name})"
    subprocess.run(
        [str(paths.bin_file), "argument with spaces", injected_argument],
        check=True,
        cwd=tmp_path,
        timeout=5,
    )

    assert captured.read_text(encoding="utf-8").splitlines() == [
        str(paths.installed_source),
        "argument with spaces",
        injected_argument,
    ]
    assert not marker.exists()
    assert paths.bin_file.read_text(encoding="utf-8").startswith("#!/bin/sh\n")


def test_desktop_shortcut_installs_interactive_exec(tmp_path, monkeypatch):
    paths = app_paths(tmp_path)
    desktop_dir = tmp_path / "Desktop"
    desktop_dir.mkdir()
    monkeypatch.setattr(crosshair, "_desktop_dir", lambda: desktop_dir)
    monkeypatch.setattr(crosshair, "_validate_desktop_file", lambda _path: None)

    shortcut = crosshair.install_desktop_shortcut(paths)

    content = shortcut.read_text(encoding="utf-8")
    assert f'Exec="{paths.bin_file}"' in content
    assert "--daemon" not in content
    assert paths.installed_source.is_file()
    installed_core = paths.installed_source.with_name("crosshair_core.py")
    source_core = Path(crosshair.__file__).with_name("crosshair_core.py")
    assert installed_core.read_bytes() == source_core.read_bytes()
    assert paths.bin_file.is_file()


def test_install_self_refuses_symlink_core_module(tmp_path, monkeypatch):
    source_dir = tmp_path / "source"
    source_dir.mkdir()
    source = source_dir / "crosshair.py"
    source.write_text("# frontend\\n", encoding="utf-8")
    real_core = tmp_path / "real_core.py"
    real_core.write_text("# core\\n", encoding="utf-8")
    (source_dir / "crosshair_core.py").symlink_to(real_core)
    monkeypatch.setattr(crosshair, "__file__", str(source))

    paths = app_paths(tmp_path)
    with pytest.raises(core.CoreError, match="symlink core module"):
        crosshair.install_self(paths)

    assert not paths.installed_source.exists()
    assert not paths.installed_source.with_name("crosshair_core.py").exists()
    assert not paths.bin_file.exists()


def test_autostart_installs_headless_exec(tmp_path, monkeypatch):
    paths = app_paths(tmp_path)
    monkeypatch.setattr(crosshair, "_validate_desktop_file", lambda _path: None)
    monkeypatch.setattr(crosshair, "_remove_legacy_systemd_unit", lambda: [])

    _bin_path, autostart = crosshair.install_autostart(paths)

    content = autostart.read_text(encoding="utf-8")
    assert f'Exec="{paths.bin_file}" --daemon --no-ui' in content
    assert "NoDisplay=true" in content


def test_purge_config_recursively_removes_tree_without_following_symlinks(tmp_path):
    paths = app_paths(tmp_path)
    nested = paths.config_dir / "nested" / "deep"
    nested.mkdir(parents=True)
    (paths.config_dir / "config.json").write_text("{}", encoding="utf-8")
    (nested / "leftover.dat").write_text("remove", encoding="utf-8")
    external = tmp_path / "do-not-delete.txt"
    external.write_text("preserve", encoding="utf-8")
    (nested / "external-link").symlink_to(external)

    removed = crosshair.purge_config(paths)

    assert removed == [str(paths.config_dir)]
    assert not paths.config_dir.exists()
    assert external.read_text(encoding="utf-8") == "preserve"


def test_purge_config_refuses_symlink_root(tmp_path, capsys):
    paths = app_paths(tmp_path)
    real_config = tmp_path / "real-config"
    real_config.mkdir()
    sentinel = real_config / "keep.txt"
    sentinel.write_text("preserve", encoding="utf-8")
    paths.config_dir.parent.mkdir(parents=True)
    paths.config_dir.symlink_to(real_config, target_is_directory=True)

    assert crosshair.purge_config(paths) == []
    assert sentinel.read_text(encoding="utf-8") == "preserve"
    assert "Refusing to purge symlink" in capsys.readouterr().err


def test_desktop_dir_ignores_relative_xdg_user_dir(tmp_path, monkeypatch):
    from types import SimpleNamespace

    monkeypatch.setattr(crosshair.Path, "home", classmethod(lambda _cls: tmp_path))
    monkeypatch.setattr(
        crosshair,
        "_trusted_executable",
        lambda name: "/usr/bin/xdg-user-dir" if name == "xdg-user-dir" else None,
    )
    monkeypatch.setattr(
        crosshair.subprocess,
        "run",
        lambda *_args, **_kwargs: SimpleNamespace(stdout="Desktop\\n", returncode=0),
    )

    assert crosshair._desktop_dir() == tmp_path / "Desktop"


def test_start_daemon_and_wait_retries_temporary_startup_response(
    tmp_path, monkeypatch
):
    paths = app_paths(tmp_path)
    responses = iter(
        [
            {"ok": False, "message": "daemon is still starting up"},
            {
                "ok": False,
                "message": "GTK main loop did not process the command before timeout",
            },
            {"ok": True, "pid": 1234},
        ]
    )
    monkeypatch.setattr(crosshair, "start_daemon_background", lambda _paths: True)
    monkeypatch.setattr(core, "ipc_call", lambda *_args, **_kwargs: next(responses))
    monkeypatch.setattr(crosshair.time, "sleep", lambda _seconds: None)

    assert crosshair.start_daemon_and_wait(paths, timeout=1.0) is True


def test_start_daemon_and_wait_fails_when_process_cannot_be_spawned(
    tmp_path, monkeypatch
):
    paths = app_paths(tmp_path)
    monkeypatch.setattr(crosshair, "start_daemon_background", lambda _paths: False)

    def unexpected_ipc(*_args, **_kwargs):
        raise AssertionError("IPC must not be attempted when spawn failed")

    monkeypatch.setattr(core, "ipc_call", unexpected_ipc)
    assert crosshair.start_daemon_and_wait(paths) is False


def test_start_daemon_and_wait_stops_on_permanent_startup_error(
    tmp_path, monkeypatch
):
    paths = app_paths(tmp_path)
    monkeypatch.setattr(crosshair, "start_daemon_background", lambda _paths: True)
    monkeypatch.setattr(
        core,
        "ipc_call",
        lambda *_args, **_kwargs: {"ok": False, "message": "GTK unavailable"},
    )

    assert crosshair.start_daemon_and_wait(paths, timeout=1.0) is False


def test_request_restart_reports_daemon_rejection(tmp_path, monkeypatch):
    paths = app_paths(tmp_path)
    monkeypatch.setattr(
        core,
        "ipc_call",
        lambda *_args, **_kwargs: {"ok": False, "message": "daemon is shutting down"},
    )

    success, error = crosshair.request_restart(paths)

    assert success is False
    assert error == "daemon is shutting down"


def test_request_restart_waits_for_replacement_pid(tmp_path, monkeypatch):
    paths = app_paths(tmp_path)
    responses = iter([
        {"ok": True, "pid": 100, "restarting": True},
        {"ok": True, "pid": 100},
        {"ok": True, "pid": 101},
    ])
    monkeypatch.setattr(
        core, "ipc_call", lambda *_args, **_kwargs: next(responses)
    )
    monkeypatch.setattr(crosshair.time, "sleep", lambda _seconds: None)

    assert crosshair.request_restart(paths, timeout=1.0) == (True, None)


def test_status_cli_reports_unresponsive_daemon(tmp_path, monkeypatch, capsys):
    paths = app_paths(tmp_path)
    monkeypatch.setattr(
        core.AppPaths,
        "from_env",
        classmethod(lambda _cls: paths),
    )
    monkeypatch.setattr(
        core,
        "ipc_call",
        lambda *_args, **_kwargs: {
            "ok": False,
            "message": "GTK main loop did not process the command before timeout",
        },
    )
    monkeypatch.setattr(sys, "argv", ["crosshair", "--status"])

    assert crosshair.main() == 2
    assert "unresponsive" in capsys.readouterr().err


def test_default_launch_does_not_try_duplicate_daemon_when_activation_fails(
    tmp_path, monkeypatch, capsys
):
    paths = app_paths(tmp_path)
    monkeypatch.setattr(
        core.AppPaths,
        "from_env",
        classmethod(lambda _cls: paths),
    )
    monkeypatch.setattr(
        core,
        "ipc_call",
        lambda *_args, **_kwargs: {
            "ok": False,
            "message": "GTK main loop did not process the command before timeout",
        },
    )
    monkeypatch.setattr(sys, "argv", ["crosshair"])

    def unexpected_start(*_args, **_kwargs):
        raise AssertionError("must not try a duplicate daemon while the lock is held")

    monkeypatch.setattr(crosshair, "run_daemon", unexpected_start)
    assert crosshair.main() == 2
    assert "Could not open Crosshair settings" in capsys.readouterr().err


def test_request_restart_fails_if_replacement_never_appears(tmp_path, monkeypatch):
    paths = app_paths(tmp_path)
    monkeypatch.setattr(
        core, "ipc_call", lambda *_args, **_kwargs: {"ok": True, "pid": 100}
    )
    monkeypatch.setattr(crosshair.time, "sleep", lambda _seconds: None)

    success, error = crosshair.request_restart(paths, timeout=0.01)

    assert success is False
    assert "replacement daemon did not become ready" in error


def test_request_restart_starts_daemon_when_not_running(tmp_path, monkeypatch):
    paths = app_paths(tmp_path)

    def unavailable(*_args, **_kwargs):
        raise core.IpcUnavailable("no socket")

    monkeypatch.setattr(core, "ipc_call", unavailable)
    monkeypatch.setattr(crosshair, "start_daemon_and_wait", lambda _paths: True)

    assert crosshair.request_restart(paths) == (True, None)


def test_request_restart_reports_failed_cold_start(tmp_path, monkeypatch):
    paths = app_paths(tmp_path)

    def unavailable(*_args, **_kwargs):
        raise core.IpcUnavailable("no socket")

    monkeypatch.setattr(core, "ipc_call", unavailable)
    monkeypatch.setattr(crosshair, "start_daemon_and_wait", lambda _paths: False)

    success, error = crosshair.request_restart(paths)

    assert success is False
    assert error == "Could not start or contact the Crosshair daemon."


def test_legacy_config_migration_keeps_directory_private(tmp_path):
    paths = app_paths(tmp_path)
    old_config = paths.home / ".config" / "purplecrosshair" / "config.json"
    old_config.parent.mkdir(parents=True)
    old_config.write_text('{"enabled": false}', encoding="utf-8")

    crosshair._migrate_legacy_config(paths, core.setup_logging(paths))

    assert paths.config_file.read_text(encoding="utf-8") == '{"enabled": false}'
    assert paths.config_dir.stat().st_mode & 0o777 == 0o700
    assert paths.config_file.stat().st_mode & 0o777 == 0o600


def test_legacy_config_migration_refuses_symlink_destination(tmp_path):
    paths = app_paths(tmp_path)
    old_config = paths.home / ".config" / "purplecrosshair" / "config.json"
    old_config.parent.mkdir(parents=True)
    old_config.write_text('{"enabled": false}', encoding="utf-8")

    real_config = tmp_path / "real-config"
    real_config.mkdir()
    sentinel = real_config / "keep.txt"
    sentinel.write_text("preserve", encoding="utf-8")
    paths.config_dir.parent.mkdir(parents=True)
    paths.config_dir.symlink_to(real_config, target_is_directory=True)

    crosshair._migrate_legacy_config(paths, core.setup_logging(paths))

    assert paths.config_dir.is_symlink()
    assert not (real_config / "config.json").exists()
    assert sentinel.read_text(encoding="utf-8") == "preserve"
