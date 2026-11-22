//! Watchdog (It1.4, P5): cuándo se ha cerrado el juego.
//!
//! Quien vigila (la GUI) escanea los procesos cada poco y, cuando esto da `false`, llama a
//! [`crate::Engine::revert_session`]: se deshace lo de sesión y lo persistente queda hasta
//! Restaurar.

use framerush_platform::{ProcessInfo, Target};

/// Sigue en marcha algún proceso del juego. Cuenta el `(pid, start_time)`: un PID reutilizado
/// por otro proceso no es el juego.
pub fn game_running(game: &[Target], processes: &[ProcessInfo]) -> bool {
    game.iter().any(|g| {
        processes
            .iter()
            .any(|p| p.target.pid == g.pid && p.target.start_time == g.start_time)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_platform::StartTime;
    use std::path::PathBuf;

    fn target(pid: u32, start: u64) -> Target {
        Target {
            pid,
            start_time: StartTime(start),
            exe_path: PathBuf::from("/game/game.exe"),
        }
    }

    fn running(targets: &[Target]) -> Vec<ProcessInfo> {
        targets
            .iter()
            .map(|t| ProcessInfo {
                target: t.clone(),
                name: "game.exe".into(),
                memory_bytes: 0,
                cpu_percent: None,
                own: Some(true),
                framerush: false,
            })
            .collect()
    }

    #[test]
    fn the_game_runs_while_any_of_its_processes_does() {
        let game = [target(9, 100), target(10, 101)];
        assert!(game_running(&game, &running(&[target(10, 101)])));
        assert!(!game_running(&game, &running(&[])));
        // Sin juego no hay nada que vigilar.
        assert!(!game_running(&[], &running(&[target(9, 100)])));
    }

    #[test]
    fn a_reused_pid_is_not_the_game() {
        let game = [target(9, 100)];
        assert!(!game_running(&game, &running(&[target(9, 555)])));
    }
}
