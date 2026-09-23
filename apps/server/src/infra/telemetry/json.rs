use std::io::Write;

use serde_json::{Map, Number, Value};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

const TIMESTAMP: &str = "timestamp";
const LEVEL: &str = "level";
const TARGET: &str = "target";
const RESERVED: [&str; 3] = [TIMESTAMP, LEVEL, TARGET];

// tracing-subscriber's built-in JSON formatter nests span fields under
// `span`/`spans`; ARCHITECTURE §14.2 requires `request_id` and the other
// canonical fields at the top level of every line, so span fields are
// flattened here instead.
pub struct JsonLayer<W, T> {
    make_writer: W,
    timer: T,
}

impl<W, T> JsonLayer<W, T> {
    pub const fn new(make_writer: W, timer: T) -> Self {
        Self { make_writer, timer }
    }
}

impl<W, T: FormatTime> JsonLayer<W, T> {
    fn render(&self, metadata: &Metadata<'_>, fields: &Map<String, Value>) -> Vec<u8> {
        let mut timestamp = String::new();
        let timestamp = match self.timer.format_time(&mut Writer::new(&mut timestamp)) {
            Ok(()) => Value::String(timestamp),
            Err(_) => Value::Null,
        };
        let mut line = Vec::with_capacity(256);
        line.push(b'{');
        push_entry(&mut line, TIMESTAMP, &timestamp);
        push_entry(&mut line, LEVEL, &Value::from(metadata.level().as_str()));
        push_entry(&mut line, TARGET, &Value::from(metadata.target()));
        for (key, value) in fields {
            if !RESERVED.contains(&key.as_str()) {
                push_entry(&mut line, key, value);
            }
        }
        line.extend_from_slice(b"}\n");
        line
    }
}

fn push_entry(line: &mut Vec<u8>, key: &str, value: &Value) {
    if line.len() > 1 {
        line.push(b',');
    }
    if serde_json::to_writer(&mut *line, key).is_ok() {
        line.push(b':');
        if serde_json::to_writer(&mut *line, value).is_err() {
            line.extend_from_slice(b"null");
        }
    }
}

struct SpanFields(Map<String, Value>);

struct FieldVisitor<'a>(&'a mut Map<String, Value>);

impl FieldVisitor<'_> {
    fn insert(&mut self, field: &Field, value: Value) {
        self.0.insert(field.name().to_owned(), value);
    }
}

impl Visit for FieldVisitor<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        let value =
            Number::from_f64(value).map_or_else(|| Value::from(value.to_string()), Value::Number);
        self.insert(field, value);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::from(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.insert(field, Value::from(format!("{value:?}")));
    }
}

impl<S, W, T> Layer<S> for JsonLayer<W, T>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> MakeWriter<'w> + 'static,
    T: FormatTime + 'static,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut fields = Map::new();
        attrs.record(&mut FieldVisitor(&mut fields));
        span.extensions_mut().insert(SpanFields(fields));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        if let Some(SpanFields(fields)) = extensions.get_mut::<SpanFields>() {
            values.record(&mut FieldVisitor(fields));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Map::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(SpanFields(span_fields)) = span.extensions().get::<SpanFields>() {
                    fields.extend(
                        span_fields
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone())),
                    );
                }
            }
        }
        event.record(&mut FieldVisitor(&mut fields));
        let line = self.render(event.metadata(), &fields);
        let _ = self
            .make_writer
            .make_writer_for(event.metadata())
            .write_all(&line);
    }
}
