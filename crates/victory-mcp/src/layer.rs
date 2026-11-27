//! A tracing layer that keeps the log lines in the ring. It is installed
//! only when `VICTORY_MCP` is set.

use std::fmt::{self, Write as _};
use std::sync::Arc;

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::Shared;

/// A kept line is cut to this many bytes.
const LINE_MAX: usize = 4096;

pub struct LogLayer {
    pub(crate) shared: Arc<Shared>,
}

struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.rest, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.rest, " {}={value:?}", field.name());
        }
    }
}

/// `line` cut to at most `max` bytes at a character boundary.
pub fn cut(mut line: String, max: usize) -> String {
    if line.len() > max {
        let mut at = max.saturating_sub(3);
        while !line.is_char_boundary(at) {
            at -= 1;
        }
        line.truncate(at);
        line.push_str("...");
    }
    line
}

/// One log line as the ring keeps it: seconds since the start, level,
/// target, message and fields.
pub fn format_line(seconds: f64, level: &str, target: &str, message: &str, fields: &str) -> String {
    let mut line = String::with_capacity(48 + message.len() + fields.len());
    let _ = write!(line, "{seconds:>9.3}s {level:>5} {target}: {message}{fields}");
    cut(line, LINE_MAX)
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut fields = Fields { message: String::new(), rest: String::new() };
        event.record(&mut fields);
        let line = format_line(
            self.shared.started.elapsed().as_secs_f64(),
            meta.level().as_str(),
            meta.target(),
            &fields.message,
            &fields.rest,
        );
        self.shared.log.push(Arc::from(line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn cuts_at_a_character_boundary() {
        let s = "é".repeat(10);
        let c = cut(s, 8);
        assert!(c.len() <= 8 && c.ends_with("..."));
        assert_eq!(cut("short".into(), 8), "short");
    }

    #[test]
    fn the_layer_keeps_events_with_their_fields() {
        let shared = Arc::new(Shared::new(crate::Config::default()));
        let subscriber = tracing_subscriber::registry().with(LogLayer { shared: shared.clone() });
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(hash = %"abc", frame = 7u64, "first bind");
            tracing::warn!("plain");
        });
        let lines = shared.log.snapshot();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(" INFO ") && lines[0].ends_with("first bind hash=abc frame=7"), "{}", lines[0]);
        assert!(lines[1].contains(" WARN ") && lines[1].ends_with("plain"), "{}", lines[1]);
    }
}
