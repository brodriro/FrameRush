//! Sondeo de los sensores en un hilo aparte (PLAN.md §2: 1 Hz, emitido como evento).

use crate::{Sensor, SensorProvider};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

/// Intervalo de sondeo por defecto (1 Hz).
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Hilo que lee todos los proveedores cada `interval` y entrega las lecturas a `on_read`. Se
/// para al soltarlo, sin esperar al siguiente intervalo.
#[derive(Debug)]
pub struct Poller {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Poller {
    pub fn spawn(
        mut providers: Vec<Box<dyn SensorProvider>>,
        interval: Duration,
        mut on_read: impl FnMut(Vec<Sensor>) + Send + 'static,
    ) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            loop {
                on_read(providers.iter_mut().flat_map(|p| p.read()).collect());
                match stopped.recv_timeout(interval) {
                    Err(RecvTimeoutError::Timeout) => {}
                    // Señal de parada o `Poller` soltado.
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        // Soltar el emisor despierta al hilo (Disconnected).
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Reading};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    struct Fake(u32);

    impl SensorProvider for Fake {
        fn read(&mut self) -> Vec<Sensor> {
            self.0 += 1;
            vec![Sensor {
                id: "fake".into(),
                label: crate::Label::Named("Fake".into()),
                kind: Kind::GpuTemperature,
                reading: Some(Reading::Celsius(self.0 as f32)),
            }]
        }
    }

    #[test]
    fn polls_repeatedly_and_stops_without_waiting_for_the_interval() {
        let seen = Arc::new(Mutex::new(vec![]));
        let sink = Arc::clone(&seen);
        let poller = Poller::spawn(
            vec![Box::new(Fake(0))],
            Duration::from_millis(10),
            move |sensors| sink.lock().unwrap().extend(sensors),
        );
        let start = Instant::now();
        while seen.lock().unwrap().len() < 3 {
            assert!(start.elapsed() < Duration::from_secs(5), "no sondea");
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(poller);
        let readings: Vec<_> = seen.lock().unwrap().iter().map(|s| s.reading).collect();
        assert_eq!(
            readings[..3],
            [1.0, 2.0, 3.0].map(|c| Some(Reading::Celsius(c)))
        );

        // Con un intervalo largo, soltarlo no espera al siguiente.
        let poller = Poller::spawn(vec![Box::new(Fake(0))], Duration::from_secs(60), |_| {});
        let start = Instant::now();
        drop(poller);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
