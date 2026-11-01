from __future__ import annotations

import io
import sys

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
    assert paths.bin_file.is_file()


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
    monkeypatch.setattr(crosshair.shutil, "which", lambda _command: "/bin/xdg-user-dir")
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
