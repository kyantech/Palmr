use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context as LayerContext, Layer};

#[derive(Clone, Default)]
pub struct Recorded(Arc<Mutex<Captured>>);

#[derive(Default)]
pub struct Captured {
    statements: Vec<String>,
    transactions: Vec<(String, u64)>,
}

impl Recorded {
    pub fn clear(&self) {
        let mut captured = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        captured.statements.clear();
        captured.transactions.clear();
    }

    pub fn statements(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .statements
            .clone()
    }

    pub fn transactions(&self) -> Vec<(String, u64)> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .transactions
            .clone()
    }
}

pub struct Recorder(pub Recorded);

#[derive(Default)]
struct Fields {
    text: String,
    transaction: Option<String>,
    elapsed_ms: Option<u64>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "summary" | "db.statement" => {
                self.text.push(' ');
                self.text.push_str(value);
            }
            "transaction" => self.transaction = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "elapsed_ms" {
            self.elapsed_ms = Some(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "transaction" && self.transaction.is_none() {
            self.transaction = Some(format!("{value:?}").trim_matches('"').to_owned());
        }
    }
}

impl<S: Subscriber> Layer<S> for Recorder {
    fn on_event(&self, event: &Event<'_>, _context: LayerContext<'_, S>) {
        let target = event.metadata().target();
        let mut fields = Fields::default();
        event.record(&mut fields);
        let mut captured = self.0 .0.lock().unwrap_or_else(PoisonError::into_inner);
        if target == "sqlx::query" {
            captured.statements.push(fields.text);
        } else if let (Some(name), Some(elapsed)) = (fields.transaction, fields.elapsed_ms) {
            captured.transactions.push((name, elapsed));
        }
    }
}

pub fn about(statements: &[String], needle: &str) -> usize {
    statements
        .iter()
        .filter(|statement| statement.contains(needle))
        .count()
}
