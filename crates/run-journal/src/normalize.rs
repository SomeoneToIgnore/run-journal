use runvil::harness::Record;
use serde_json::Value;

pub(crate) fn normalize(value: &mut Value) {
    match value {
        Value::String(text) if text.contains(GUARD) => *text = scrub(text),
        Value::Array(items) => items.iter_mut().for_each(normalize),
        Value::Object(fields) => fields.values_mut().for_each(normalize),
        _ => {}
    }
}

pub(crate) fn render(record: &Record) -> String {
    let payload = record.payload.to_string();
    let shown = match payload.char_indices().nth(SHOWN) {
        Some((end, _)) => format!("{}...", &payload[..end]),
        None => payload,
    };
    format!(
        "{} {}:{} {shown}",
        record.kind.as_str(),
        record.task_id,
        record.task_seq
    )
}

const GUARD: &str = "untrusted_input_";

const NONCE: usize = 32;

const SHOWN: usize = 200;

fn scrub(text: &str) -> String {
    let mut scrubbed = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(GUARD) {
        let (head, tail) = rest.split_at(at + GUARD.len());
        scrubbed.push_str(head);
        let digits = tail
            .bytes()
            .take(NONCE)
            .take_while(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            .count();
        if digits == NONCE {
            scrubbed.push_str(&"0".repeat(NONCE));
            rest = &tail[NONCE..];
        } else {
            rest = tail;
        }
    }
    scrubbed.push_str(rest);
    scrubbed
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::normalize;

    #[test]
    fn a_guard_nonce_reads_the_same_whatever_seed_made_it() {
        let mut first = json!({
            "messages": [
                "untrusted_input_0123456789abcdef0123456789abcdef says <untrusted_input_0123456789abcdef0123456789abcdef>",
                "untrusted_input_short and untrusted_input_0123456789ABCDEF0123456789abcdef stay",
            ],
        });
        let mut second = json!({
            "messages": [
                "untrusted_input_ffffffffffffffffffffffffffffffff says <untrusted_input_ffffffffffffffffffffffffffffffff>",
                "untrusted_input_short and untrusted_input_0123456789ABCDEF0123456789abcdef stay",
            ],
        });
        normalize(&mut first);
        normalize(&mut second);
        assert_eq!(first, second);
        assert_eq!(
            first["messages"][0],
            "untrusted_input_00000000000000000000000000000000 says <untrusted_input_00000000000000000000000000000000>"
        );
    }
}
