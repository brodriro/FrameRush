//! Tests de integración de la CLI: nunca aplican cambios al sistema.

use std::process::Command;

fn framerush(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_framerush"))
        .args(args)
        .output()
        .expect("se puede lanzar el binario");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn plan_is_a_dry_run_for_every_level() {
    for level in ["normal", "agresivo", "extremo"] {
        let (ok, stdout) = framerush(&["plan", "--level", level]);
        assert!(ok, "{level}");
        assert!(
            stdout.contains(&format!("nivel {level} (dry-run")),
            "{stdout}"
        );
    }
}

#[test]
fn unknown_level_is_rejected() {
    let (ok, _) = framerush(&["plan", "--level", "turbo"]);
    assert!(!ok);
}

#[test]
fn scan_lists_processes() {
    let (ok, stdout) = framerush(&["scan"]);
    assert!(ok);
    assert!(stdout.trim_end().ends_with("procesos"));
}

/// Lanza la CLI con un journal en una carpeta temporal propia del test.
fn framerush_in(dir: &std::path::Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_framerush"))
        .env("FRAMERUSH_DATA_DIR", dir)
        .args(args)
        .output()
        .expect("se puede lanzar el binario");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("framerush-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn status_without_a_session() {
    let dir = temp_dir("status");
    let (ok, stdout) = framerush_in(&dir, &["status"]);
    assert!(ok);
    assert!(stdout.contains("no hay sesión abierta"), "{stdout}");
    let (ok, stdout) = framerush_in(&dir, &["status", "--json"]);
    assert!(ok);
    assert!(stdout.contains("\"session\": []"), "{stdout}");
    assert!(!dir.join("session.json").exists());
}

#[test]
fn apply_without_yes_changes_nothing() {
    let dir = temp_dir("apply");
    let (ok, stdout) = framerush_in(&dir, &["apply", "--level", "extremo"]);
    assert!(ok);
    assert!(stdout.contains("no se aplica sin --yes"), "{stdout}");
    assert!(!dir.join("session.json").exists());
}

#[test]
fn unknown_rule_in_include_is_rejected() {
    let dir = temp_dir("include");
    let (ok, _) = framerush_in(&dir, &["apply", "--include", "no-existe"]);
    assert!(!ok);
}

#[cfg(windows)]
#[test]
fn revert_without_session_does_nothing() {
    let dir = temp_dir("revert");
    for cmd in ["revert", "undo"] {
        let (ok, stdout) = framerush_in(&dir, &[cmd]);
        assert!(ok, "{cmd}");
        assert!(stdout.contains("No había nada que hacer"), "{stdout}");
    }
}

#[test]
fn apply_shows_the_game_without_touching_it() {
    let dir = temp_dir("game");
    let pid = std::process::id().to_string();
    let (ok, stdout) = framerush_in(&dir, &["apply", "--game", &pid]);
    assert!(ok);
    assert!(
        stdout.contains(&format!("(pid {pid}) → prioridad Alta")),
        "{stdout}"
    );
    assert!(!dir.join("session.json").exists());
}

#[test]
fn realtime_needs_a_second_confirmation() {
    let dir = temp_dir("realtime");
    let pid = std::process::id().to_string();
    let (ok, _) = framerush_in(
        &dir,
        &["apply", "--game", &pid, "--game-priority", "realtime"],
    );
    assert!(!ok);
}

#[test]
fn unknown_game_is_rejected() {
    let dir = temp_dir("nogame");
    let (ok, _) = framerush_in(&dir, &["plan", "--game", "no-existe-framerush.exe"]);
    assert!(!ok);
}

/// Un `ping` copiado como `EasyAntiCheat.exe`: un anticheat falso, propio del test.
#[cfg(windows)]
#[test]
fn a_running_anticheat_is_reported() {
    let dir = temp_dir("anticheat");
    let fake = dir.join("EasyAntiCheat.exe");
    let windir = std::env::var_os("SystemRoot").expect("Windows define SystemRoot");
    std::fs::copy(
        std::path::Path::new(&windir).join(r"System32\PING.EXE"),
        &fake,
    )
    .unwrap();
    let mut child = Command::new(&fake)
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let (ok, stdout) = framerush_in(&dir, &["plan"]);
    let _ = child.kill();
    let _ = child.wait();
    assert!(ok);
    assert!(
        stdout.contains("Anticheat en ejecución: EasyAntiCheat.exe"),
        "{stdout}"
    );
}

#[cfg(windows)]
#[test]
fn tweaks_are_listed_and_can_be_excluded() {
    let dir = temp_dir("tweaks");
    let (ok, stdout) = framerush_in(&dir, &["apply", "--exclude", "game-dvr-off"]);
    assert!(ok);
    assert!(stdout.contains("[ ] game-dvr-off"), "{stdout}");
    assert!(stdout.contains("[x] windows-game-mode"), "{stdout}");
    assert!(!dir.join("session.json").exists());
}

/// Sin `--admin`, la directiva de Game DVR sale con candado y no se puede marcar (P14).
#[cfg(windows)]
#[test]
fn admin_tweaks_are_locked_without_admin() {
    let dir = temp_dir("admin-tweaks");
    let (ok, stdout) = framerush_in(&dir, &["plan", "--level", "agresivo"]);
    assert!(ok);
    assert!(
        stdout.contains("[ ] game-dvr-off-policy") && stdout.contains("requiere admin"),
        "{stdout}"
    );
    let (ok, stdout) = framerush_in(&dir, &["plan", "--level", "agresivo", "--admin"]);
    assert!(ok);
    assert!(stdout.contains("[x] game-dvr-off-policy"), "{stdout}");
    let args = [
        "apply",
        "--level",
        "agresivo",
        "--include",
        "game-dvr-off-policy",
    ];
    let (ok, _) = framerush_in(&dir, &args);
    assert!(!ok);
    assert!(!dir.join("session.json").exists());
}

/// P1: la prioridad persistente sale desde Agresivo, con candado sin `--admin`, nunca con
/// prioridad Normal y se desmarca con `--exclude`.
#[cfg(windows)]
#[test]
fn persistent_priority_follows_level_admin_and_priority() {
    let dir = temp_dir("persistent-priority");
    let pid = std::process::id().to_string();
    let game = ["--game", pid.as_str()];
    let run = |extra: &[&str]| {
        let args = [&["plan"][..], &game, extra].concat();
        let (ok, stdout) = framerush_in(&dir, &args);
        assert!(ok, "{stdout}");
        stdout
    };
    assert!(!run(&["--level", "normal", "--admin"]).contains("game-priority-persistent"));
    let locked = run(&["--level", "agresivo"]);
    assert!(locked.contains("[ ] game-priority-persistent"), "{locked}");
    assert!(locked.contains("requiere admin"), "{locked}");
    let on = run(&["--level", "agresivo", "--admin"]);
    assert!(on.contains("[x] game-priority-persistent"), "{on}");
    let normal = run(&["--level", "extremo", "--admin", "--game-priority", "normal"]);
    assert!(!normal.contains("game-priority-persistent"), "{normal}");

    let args = ["apply", "--level", "agresivo", "--admin", "--game", &pid];
    let args = [&args[..], &["--exclude", "game-priority-persistent"]].concat();
    let (ok, stdout) = framerush_in(&dir, &args);
    assert!(ok, "{stdout}");
    assert!(stdout.contains("[ ] game-priority-persistent"), "{stdout}");
    assert!(!dir.join("session.json").exists());
}

/// `mmcss-games` es opt-in: sale desmarcado y `--include` lo marca.
#[cfg(windows)]
#[test]
fn optional_tweaks_need_include() {
    let dir = temp_dir("optional-tweaks");
    let base = ["apply", "--level", "agresivo", "--admin"];
    let (ok, stdout) = framerush_in(&dir, &base);
    assert!(ok);
    assert!(stdout.contains("[ ] mmcss-games"), "{stdout}");
    let (ok, stdout) = framerush_in(&dir, &[&base[..], &["--include", "mmcss-games"]].concat());
    assert!(ok);
    assert!(stdout.contains("[x] mmcss-games"), "{stdout}");
    assert!(!dir.join("session.json").exists());
}

#[test]
fn gpu_alto_needs_a_game() {
    let dir = temp_dir("gpu");
    let (ok, _) = framerush_in(&dir, &["apply", "--gpu-alto"]);
    assert!(!ok);
    let pid = std::process::id().to_string();
    let (ok, stdout) = framerush_in(&dir, &["apply", "--game", &pid, "--gpu-alto"]);
    assert!(ok);
    assert!(stdout.contains("[x] gpu-preference = alto"), "{stdout}");
    assert!(!dir.join("session.json").exists());
}

#[test]
fn fso_off_needs_a_game() {
    let dir = temp_dir("fso");
    let (ok, _) = framerush_in(&dir, &["apply", "--fso-off"]);
    assert!(!ok);
    let pid = std::process::id().to_string();
    let (ok, stdout) = framerush_in(&dir, &["apply", "--game", &pid, "--fso-off"]);
    assert!(ok);
    assert!(
        stdout.contains("[x] fullscreen-optimizations-off"),
        "{stdout}"
    );
    assert!(!dir.join("session.json").exists());
}
