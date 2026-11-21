//! Tests de integración de la CLI: solo comandos de lectura, que no tocan el sistema.

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
