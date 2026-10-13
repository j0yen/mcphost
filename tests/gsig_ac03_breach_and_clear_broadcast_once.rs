//! PRD-mcphost-golden-signals-self-probe AC3 (P0): a signal that goes red and
//! stays broadcasts exactly one `SignalBreach` once `for_s` elapses, a second
//! subscriber sees the same event, and returning to green broadcasts exactly
//! one `SignalClear`.

use mcphost::signals::{Signal, SignalEvent, Signals};

fn drain(rx: &mut tokio::sync::broadcast::Receiver<SignalEvent>) -> Vec<SignalEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

#[test]
fn red_for_for_s_breaches_once_to_every_subscriber_then_clears_once() {
    let signals = Signals::compiled();
    let mut first = signals.subscribe();
    let mut second = signals.subscribe();
    let for_s = signals.config().threshold(Signal::SqliteWriteMs).for_s;
    let t0 = 1_000_000;

    // Red from t0, sampled every 30 s, for twice as long as `for_s`.
    let mut t = t0;
    while t - t0 < 2 * for_s {
        signals.record(Signal::SqliteWriteMs, t, 900.0);
        if t - t0 < for_s {
            assert!(drain(&mut first).is_empty(), "no breach before for_s (t+{})", t - t0);
        }
        t += 30;
    }

    let breaches = drain(&mut first);
    assert_eq!(breaches.len(), 1, "exactly one breach: {breaches:?}");
    assert!(
        matches!(&breaches[0], SignalEvent::Breach { signal: Signal::SqliteWriteMs, since, .. } if *since == t0),
        "{breaches:?}"
    );
    assert_eq!(drain(&mut second), breaches, "the second subscriber receives the same event");

    // Still amber: the breach stays open, nothing is emitted.
    signals.record(Signal::SqliteWriteMs, t, 80.0);
    assert!(drain(&mut first).is_empty());
    // Below amber: exactly one clear.
    signals.record(Signal::SqliteWriteMs, t + 30, 5.0);
    signals.record(Signal::SqliteWriteMs, t + 60, 5.0);
    let clears = drain(&mut first);
    assert_eq!(clears.len(), 1, "exactly one clear: {clears:?}");
    assert!(matches!(&clears[0], SignalEvent::Clear { signal: Signal::SqliteWriteMs, .. }), "{clears:?}");
    assert_eq!(drain(&mut second), clears);
}
