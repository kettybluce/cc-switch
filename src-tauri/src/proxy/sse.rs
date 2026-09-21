#[inline]
pub(crate) fn strip_sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    // BOM / 行首空白：嗅探器与 handlers::sse_block_parts（C4）都接受缩进 `data:`。
    let line = line.trim_start_matches('\u{feff}').trim_start();
    line.strip_prefix(&format!("{field}: "))
        .or_else(|| line.strip_prefix(&format!("{field}:")))
}

/// WHATWG HTML SSE：一行结束于 `\n`、`\r\n` 或孤立 `\r`。
///
/// `at_eof=false` 时，缓冲末尾的 `\r` 可能是未完成的 `\r\n`，不能当行结束，
/// 否则随后到达的 `\n` 会被当成第二条行结束，把一次 CRLF 拆成空事件。
fn sse_line_ending_len(bytes: &[u8], i: usize, at_eof: bool) -> Option<usize> {
    match bytes.get(i) {
        Some(b'\n') => Some(1),
        Some(b'\r') => {
            if bytes.get(i + 1) == Some(&b'\n') {
                Some(2)
            } else if i + 1 < bytes.len() || at_eof {
                Some(1)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 取出下一个完整 SSE 事件（两条连续行结束符）。`at_eof` 时允许把尾部孤立 `\r` 当行结束。
fn take_sse_block_inner(buffer: &mut String, at_eof: bool) -> Option<String> {
    let bytes = buffer.as_bytes();
    let mut i = 0usize;
    let mut first_eol_start: Option<usize> = None;

    while i < bytes.len() {
        match sse_line_ending_len(bytes, i, at_eof) {
            Some(len) => {
                if let Some(start) = first_eol_start {
                    let block = buffer[..start].to_string();
                    buffer.drain(..i + len);
                    return Some(block);
                }
                first_eol_start = Some(i);
                i += len;
            }
            None => {
                if bytes[i] == b'\r' && i + 1 == bytes.len() && !at_eof {
                    // Trailing \r may still grow into \r\n — unless we already
                    // saw one line ending: a second \r is a complete delimiter
                    // (`\r\r`, or `\n\r` that later gains `\n` as a harmless leftover LF).
                    if let Some(start) = first_eol_start {
                        let block = buffer[..start].to_string();
                        buffer.drain(..i + 1);
                        return Some(block);
                    }
                    return None;
                }
                first_eol_start = None;
                i += 1;
            }
        }
    }
    None
}

#[inline]
pub(crate) fn take_sse_block(buffer: &mut String) -> Option<String> {
    take_sse_block_inner(buffer, false)
}

/// 流结束：先按完整分隔符取块，再把缺尾部空行的残余当作最后一块。
pub(crate) fn take_sse_block_eof(buffer: &mut String) -> Option<String> {
    if let Some(block) = take_sse_block_inner(buffer, true) {
        return Some(block);
    }
    let rest = std::mem::take(buffer);
    if rest.trim().is_empty() {
        None
    } else {
        Some(rest)
    }
}

/// Parsed SSE event fields used for usage collection and Last-Event-ID reconnect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
    pub retry_ms: Option<u64>,
}

impl SseEvent {
    pub(crate) fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }
}

/// Parse one SSE block: join multi-line `data:`, keep `id` / `retry` for reconnect, skip comments.
pub(crate) fn parse_sse_event(block: &str) -> SseEvent {
    let mut event = SseEvent::default();
    let mut data_lines: Vec<&str> = Vec::new();

    for line in block.trim_start_matches('\u{feff}').lines() {
        let line = line.trim_start_matches('\u{feff}');
        let trimmed_start = line.trim_start();
        if trimmed_start.is_empty() || trimmed_start.starts_with(':') {
            continue;
        }
        if let Some(value) = strip_sse_field(line, "event") {
            event.event = Some(value.to_string());
        } else if let Some(value) = strip_sse_field(line, "data") {
            data_lines.push(value);
        } else if let Some(value) = strip_sse_field(line, "id") {
            event.id = Some(value.to_string());
        } else if let Some(value) = strip_sse_field(line, "retry") {
            event.retry_ms = value.trim().parse().ok();
        }
    }

    event.data = data_lines.join("\n");
    event
}

/// Select the n=1 / `index==0` OpenAI chat choice. `.first()` is wrong when a
/// chunk lists `index:1` before `index:0`, or only contains n>1 completions.
pub(crate) fn openai_primary_choice(choices: &[serde_json::Value]) -> Option<&serde_json::Value> {
    choices.iter().find(|choice| {
        choice
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            == 0
    })
}

/// Flush a trailing incomplete UTF-8 sequence into `buffer` (lossy at true EOF).
pub(crate) fn flush_utf8_remainder(buffer: &mut String, remainder: &mut Vec<u8>) {
    if remainder.is_empty() {
        return;
    }
    buffer.push_str(&String::from_utf8_lossy(remainder));
    remainder.clear();
}

/// Append raw bytes to a UTF-8 `String` buffer, correctly handling multi-byte
/// characters that are split across chunk boundaries.
///
/// `remainder` accumulates trailing bytes from the previous chunk that form an
/// incomplete UTF-8 sequence (at most 3 bytes under normal operation). On each
/// call the remainder is prepended to `new_bytes`, the longest valid UTF-8
/// prefix is appended to `buffer`, and any trailing incomplete bytes are saved
/// back into `remainder` for the next call.
///
/// A defensive guard discards `remainder` via lossy conversion if it ever
/// exceeds 3 bytes, which cannot happen with well-formed UTF-8 streams.
pub(crate) fn append_utf8_safe(buffer: &mut String, remainder: &mut Vec<u8>, new_bytes: &[u8]) {
    // Build the byte slice to decode: prepend any leftover bytes from previous chunk.
    let (owned, bytes): (Option<Vec<u8>>, &[u8]) = if remainder.is_empty() {
        (None, new_bytes)
    } else {
        // Defensive guard: remainder should never exceed 3 bytes (max incomplete
        // UTF-8 sequence is 3 bytes: a 4-byte char missing its last byte). If it
        // does, the stream is producing genuinely invalid bytes; flush them lossy
        // and start fresh.
        if remainder.len() > 3 {
            buffer.push_str(&String::from_utf8_lossy(remainder));
            remainder.clear();
            (None, new_bytes)
        } else {
            let mut combined = std::mem::take(remainder);
            combined.extend_from_slice(new_bytes);
            (Some(combined), &[])
        }
    };
    let input = owned.as_deref().unwrap_or(bytes);

    // Decode loop: consume all valid UTF-8 and any genuinely invalid bytes,
    // only leaving a trailing incomplete sequence in remainder.
    let mut pos = 0;
    loop {
        match std::str::from_utf8(&input[pos..]) {
            Ok(s) => {
                buffer.push_str(s);
                // Everything consumed – remainder stays empty.
                return;
            }
            Err(e) => {
                let valid_up_to = pos + e.valid_up_to();
                let valid_slice = &input[pos..valid_up_to];
                match std::str::from_utf8(valid_slice) {
                    Ok(valid) => buffer.push_str(valid),
                    Err(_) => buffer.push_str(&String::from_utf8_lossy(valid_slice)),
                }
                if let Some(invalid_len) = e.error_len() {
                    // Genuinely invalid byte(s) – emit U+FFFD and continue.
                    buffer.push('\u{FFFD}');
                    pos = valid_up_to + invalid_len;
                } else {
                    // Incomplete trailing sequence – stash for next chunk.
                    *remainder = input[valid_up_to..].to_vec();
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_utf8_safe, flush_utf8_remainder, openai_primary_choice, parse_sse_event,
        strip_sse_field, take_sse_block, take_sse_block_eof,
    };

    #[test]
    fn strip_sse_field_accepts_optional_space() {
        assert_eq!(
            strip_sse_field("data: {\"ok\":true}", "data"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            strip_sse_field("data:{\"ok\":true}", "data"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            strip_sse_field("event: message_start", "event"),
            Some("message_start")
        );
        assert_eq!(
            strip_sse_field("event:message_start", "event"),
            Some("message_start")
        );
        assert_eq!(strip_sse_field("id:1", "data"), None);
        assert_eq!(
            strip_sse_field("\u{feff}data: {\"ok\":true}", "data"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            strip_sse_field("  data: {\"ok\":true}", "data"),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn take_sse_block_supports_lf_delimiters() {
        let mut buffer = "data: {\"ok\":true}\n\nrest".to_string();

        assert_eq!(
            take_sse_block(&mut buffer),
            Some("data: {\"ok\":true}".to_string())
        );
        assert_eq!(buffer, "rest");
    }

    #[test]
    fn take_sse_block_supports_crlf_delimiters() {
        let mut buffer = "data: {\"ok\":true}\r\n\r\nrest".to_string();

        assert_eq!(
            take_sse_block(&mut buffer),
            Some("data: {\"ok\":true}".to_string())
        );
        assert_eq!(buffer, "rest");
    }

    // ------------------------------------------------------------------
    // append_utf8_safe tests
    // ------------------------------------------------------------------

    #[test]
    fn ascii_passthrough() {
        let mut buf = String::new();
        let mut rem = Vec::new();
        append_utf8_safe(&mut buf, &mut rem, b"hello world");
        assert_eq!(buf, "hello world");
        assert!(rem.is_empty());
    }

    #[test]
    fn complete_multibyte_in_single_chunk() {
        let mut buf = String::new();
        let mut rem = Vec::new();
        append_utf8_safe(&mut buf, &mut rem, "你好世界".as_bytes());
        assert_eq!(buf, "你好世界");
        assert!(rem.is_empty());
    }

    #[test]
    fn split_multibyte_across_two_chunks() {
        // "你" = E4 BD A0 (3 bytes)
        let bytes = "你".as_bytes();
        assert_eq!(bytes.len(), 3);

        let mut buf = String::new();
        let mut rem = Vec::new();

        // Chunk 1: first 2 bytes (incomplete)
        append_utf8_safe(&mut buf, &mut rem, &bytes[..2]);
        assert_eq!(buf, "");
        assert_eq!(rem.len(), 2);

        // Chunk 2: last byte completes the character
        append_utf8_safe(&mut buf, &mut rem, &bytes[2..]);
        assert_eq!(buf, "你");
        assert!(rem.is_empty());
    }

    #[test]
    fn split_four_byte_char_across_chunks() {
        // 😀 = F0 9F 98 80 (4 bytes)
        let bytes = "😀".as_bytes();
        assert_eq!(bytes.len(), 4);

        let mut buf = String::new();
        let mut rem = Vec::new();

        // Send 1 byte at a time
        append_utf8_safe(&mut buf, &mut rem, &bytes[..1]);
        assert_eq!(buf, "");
        assert_eq!(rem.len(), 1);

        append_utf8_safe(&mut buf, &mut rem, &bytes[1..2]);
        assert_eq!(buf, "");
        assert_eq!(rem.len(), 2);

        append_utf8_safe(&mut buf, &mut rem, &bytes[2..3]);
        assert_eq!(buf, "");
        assert_eq!(rem.len(), 3);

        append_utf8_safe(&mut buf, &mut rem, &bytes[3..]);
        assert_eq!(buf, "😀");
        assert!(rem.is_empty());
    }

    #[test]
    fn mixed_ascii_and_split_multibyte() {
        // "hi你" = 68 69 E4 BD A0
        let all = "hi你".as_bytes();
        assert_eq!(all.len(), 5);

        let mut buf = String::new();
        let mut rem = Vec::new();

        // Chunk 1: "hi" + first byte of "你"
        append_utf8_safe(&mut buf, &mut rem, &all[..3]);
        assert_eq!(buf, "hi");
        assert_eq!(rem.len(), 1);

        // Chunk 2: remaining 2 bytes of "你"
        append_utf8_safe(&mut buf, &mut rem, &all[3..]);
        assert_eq!(buf, "hi你");
        assert!(rem.is_empty());
    }

    #[test]
    fn multiple_split_characters_in_sequence() {
        let text = "你好";
        let bytes = text.as_bytes(); // E4 BD A0 E5 A5 BD

        let mut buf = String::new();
        let mut rem = Vec::new();

        // Split in the middle: first char complete + 1 byte of second
        append_utf8_safe(&mut buf, &mut rem, &bytes[..4]);
        assert_eq!(buf, "你");
        assert_eq!(rem.len(), 1);

        // Remaining 2 bytes complete second char
        append_utf8_safe(&mut buf, &mut rem, &bytes[4..]);
        assert_eq!(buf, "你好");
        assert!(rem.is_empty());
    }

    #[test]
    fn empty_chunks_are_harmless() {
        let mut buf = String::new();
        let mut rem = Vec::new();

        append_utf8_safe(&mut buf, &mut rem, b"");
        assert_eq!(buf, "");
        assert!(rem.is_empty());

        append_utf8_safe(&mut buf, &mut rem, b"ok");
        assert_eq!(buf, "ok");

        append_utf8_safe(&mut buf, &mut rem, b"");
        assert_eq!(buf, "ok");
    }

    #[test]
    fn sse_json_with_chinese_split_at_boundary() {
        // Simulates an SSE data line with Chinese content split across chunks
        let json_line = "data: {\"text\":\"你好\"}\n\n";
        let bytes = json_line.as_bytes();

        // Find where "你" starts in the byte stream and split there
        let ni_start = bytes.windows(3).position(|w| w == "你".as_bytes()).unwrap();
        let split_point = ni_start + 1; // split inside "你"

        let mut buf = String::new();
        let mut rem = Vec::new();

        append_utf8_safe(&mut buf, &mut rem, &bytes[..split_point]);
        append_utf8_safe(&mut buf, &mut rem, &bytes[split_point..]);

        assert_eq!(buf, json_line);
        assert!(rem.is_empty());

        // Verify the buffer can be parsed as SSE with valid JSON
        let data = strip_sse_field(buf.lines().next().unwrap(), "data").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(parsed["text"], "你好");
    }

    #[test]
    fn invalid_bytes_flushed_immediately_not_accumulated() {
        // 0xFF is never valid in UTF-8 – it should be replaced immediately,
        // not stashed in remainder.
        let mut buf = String::new();
        let mut rem = Vec::new();

        // "hi" + invalid byte + "ok"
        append_utf8_safe(&mut buf, &mut rem, b"hi\xFFok");
        assert!(
            rem.is_empty(),
            "remainder should be empty after invalid byte"
        );
        assert!(buf.contains("hi"), "valid prefix must be present");
        assert!(buf.contains("ok"), "valid suffix must be present");
        assert!(buf.contains('\u{FFFD}'), "invalid byte must produce U+FFFD");
    }

    #[test]
    fn invalid_byte_in_slow_path_flushed_immediately() {
        let mut buf = String::new();
        let mut rem = Vec::new();

        // Prime remainder with an incomplete sequence (first byte of "你")
        append_utf8_safe(&mut buf, &mut rem, &"你".as_bytes()[..1]);
        assert_eq!(rem.len(), 1);

        // Next chunk starts with an invalid byte – the stale remainder and the
        // invalid byte should both be flushed, not accumulated.
        append_utf8_safe(&mut buf, &mut rem, b"\xFFworld");
        assert!(rem.is_empty(), "remainder should be empty");
        assert!(
            buf.contains("world"),
            "valid data after invalid byte must appear"
        );
    }

    #[test]
    fn defensive_guard_flushes_oversized_remainder() {
        let mut buf = String::new();
        let mut rem = Vec::new();

        // Manually inject 4 invalid bytes into remainder to trigger the >3 guard.
        // This can't happen with well-formed UTF-8, but tests the safety net.
        rem.extend_from_slice(b"\x80\x80\x80\x80");
        assert_eq!(rem.len(), 4);

        append_utf8_safe(&mut buf, &mut rem, b"hello");
        // The 4 invalid bytes should have been flushed lossy, then "hello" decoded.
        assert!(rem.is_empty(), "remainder must be empty after guard flush");
        assert!(
            buf.contains("hello"),
            "valid data after guard flush must appear"
        );
        // The 4 invalid bytes each produce a U+FFFD
        let replacement_count = buf.chars().filter(|&c| c == '\u{FFFD}').count();
        assert_eq!(
            replacement_count, 4,
            "each invalid byte should produce one U+FFFD"
        );
    }

    #[test]
    fn take_sse_block_leaves_truncated_tail_in_buffer() {
        let mut buffer = "data: {\"ok\":true}\n\ndata: {\"partial".to_string();
        assert_eq!(
            take_sse_block(&mut buffer),
            Some("data: {\"ok\":true}".to_string())
        );
        assert_eq!(buffer, "data: {\"partial");
        assert_eq!(take_sse_block(&mut buffer), None);
        assert_eq!(buffer, "data: {\"partial");
        assert_eq!(
            take_sse_block_eof(&mut buffer),
            Some("data: {\"partial".to_string())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn take_sse_block_mixed_crlf_lf_does_not_leave_cr_on_block() {
        let mut buffer = "data: {\"ok\":true}\r\n\nrest".to_string();
        assert_eq!(
            take_sse_block(&mut buffer),
            Some("data: {\"ok\":true}".to_string())
        );
        assert_eq!(buffer, "rest");
    }

    #[test]
    fn take_sse_block_lf_then_crlf() {
        let mut buffer = "data: a\n\r\ndata: b\n\n".to_string();
        assert_eq!(take_sse_block(&mut buffer), Some("data: a".to_string()));
        assert_eq!(take_sse_block(&mut buffer), Some("data: b".to_string()));
        assert!(buffer.is_empty());
    }

    #[test]
    fn take_sse_block_cr_cr_old_mac() {
        let mut buffer = "data: a\r\rdata: b\r\r".to_string();
        assert_eq!(take_sse_block(&mut buffer), Some("data: a".to_string()));
        assert_eq!(take_sse_block(&mut buffer), Some("data: b".to_string()));
        assert!(buffer.is_empty());
    }

    #[test]
    fn take_sse_block_does_not_split_pending_crlf() {
        // Trailing \r may still become \r\n. Must wait rather than treat it as EOL.
        let mut buffer = "data: {\"ok\":true}\r".to_string();
        assert_eq!(take_sse_block(&mut buffer), None);
        buffer.push('\n');
        buffer.push('\n');
        assert_eq!(
            take_sse_block(&mut buffer),
            Some("data: {\"ok\":true}".to_string())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn take_sse_block_eof_treats_trailing_cr_as_line_ending() {
        let mut buffer = "data: {\"ok\":true}\r".to_string();
        assert_eq!(
            take_sse_block_eof(&mut buffer),
            Some("data: {\"ok\":true}\r".to_string())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn take_sse_block_comment_heartbeat_does_not_eat_next_event() {
        let mut buffer = ": ping\n\nid: 7\ndata: {\"n\":1}\n\n".to_string();
        assert_eq!(take_sse_block(&mut buffer), Some(": ping".to_string()));
        assert_eq!(
            take_sse_block(&mut buffer),
            Some("id: 7\ndata: {\"n\":1}".to_string())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn parse_sse_event_reconnect_fields_and_multiline_data() {
        let event = parse_sse_event(
            "retry: 3000\nid: chunk-42\nevent: delta\ndata: {\"a\":1}\ndata: {\"b\":2}",
        );
        assert_eq!(event.retry_ms, Some(3000));
        assert_eq!(event.id.as_deref(), Some("chunk-42"));
        assert_eq!(event.event.as_deref(), Some("delta"));
        assert_eq!(event.data, "{\"a\":1}\n{\"b\":2}");
        assert!(!event.is_done());
    }

    #[test]
    fn parse_sse_event_skips_comments_and_invalid_retry() {
        let event = parse_sse_event(": OPENROUTER PROCESSING\nretry: not-a-number\ndata: [DONE]");
        assert_eq!(event.retry_ms, None);
        assert!(event.is_done());
        assert_eq!(event.data, "[DONE]");
    }

    #[test]
    fn parse_sse_event_empty_id_resets_last_event_id() {
        let event = parse_sse_event("id:\ndata: {\"ok\":true}");
        assert_eq!(event.id.as_deref(), Some(""));
        assert_eq!(event.data, "{\"ok\":true}");
    }

    #[test]
    fn parse_sse_event_strips_bom_and_indented_fields() {
        let event = parse_sse_event("\u{feff}  data: {\"ok\":true}");
        assert_eq!(event.data, "{\"ok\":true}");
    }

    #[test]
    fn openai_primary_choice_ignores_n_gt_1_when_index_0_is_not_first() {
        let choices = vec![
            serde_json::json!({"index": 1, "delta": {"content": "B"}}),
            serde_json::json!({"index": 0, "delta": {"content": "A"}}),
            serde_json::json!({"index": 2, "delta": {"content": "C"}}),
        ];
        let primary = openai_primary_choice(&choices).unwrap();
        assert_eq!(primary["delta"]["content"], "A");
    }

    #[test]
    fn openai_primary_choice_skips_chunk_with_only_index_1() {
        let choices = vec![serde_json::json!({"index": 1, "delta": {"content": "B"}})];
        assert!(openai_primary_choice(&choices).is_none());
    }

    #[test]
    fn openai_primary_choice_defaults_missing_index_to_zero() {
        let choices = vec![serde_json::json!({"delta": {"content": "hello"}})];
        let primary = openai_primary_choice(&choices).unwrap();
        assert_eq!(primary["delta"]["content"], "hello");
    }

    #[test]
    fn flush_utf8_remainder_lossy_at_eof() {
        let mut buf = String::from("hi");
        let mut rem = "你".as_bytes()[..1].to_vec();
        flush_utf8_remainder(&mut buf, &mut rem);
        assert!(rem.is_empty());
        assert!(buf.starts_with("hi"));
        assert!(buf.contains('\u{FFFD}'));
    }

    #[test]
    fn append_utf8_safe_then_take_sse_block_chinese_split_across_crlf_event() {
        let json_line = "id: evt-1\ndata: {\"text\":\"你好\"}\r\n\r\n";
        let bytes = json_line.as_bytes();
        let ni_start = bytes.windows(3).position(|w| w == "你".as_bytes()).unwrap();

        let mut buf = String::new();
        let mut rem = Vec::new();
        append_utf8_safe(&mut buf, &mut rem, &bytes[..ni_start + 1]);
        assert!(!rem.is_empty());
        append_utf8_safe(&mut buf, &mut rem, &bytes[ni_start + 1..]);
        assert!(rem.is_empty());

        let block = take_sse_block(&mut buf).expect("complete CRLF event");
        let event = parse_sse_event(&block);
        assert_eq!(event.id.as_deref(), Some("evt-1"));
        let parsed: serde_json::Value = serde_json::from_str(&event.data).unwrap();
        assert_eq!(parsed["text"], "你好");
        assert!(buf.is_empty());
    }

    #[test]
    fn bytewise_sse_stream_reassembles_emoji_and_reconnect_id() {
        let frame = "retry: 1500\nid: 99\ndata: {\"c\":\"😀\"}\n\n";
        let mut buf = String::new();
        let mut rem = Vec::new();
        let mut last_event = None;
        for byte in frame.as_bytes() {
            append_utf8_safe(&mut buf, &mut rem, &[*byte]);
            if let Some(block) = take_sse_block(&mut buf) {
                last_event = Some(parse_sse_event(&block));
            }
        }
        let event = last_event.expect("event reassembled bytewise");
        assert_eq!(event.retry_ms, Some(1500));
        assert_eq!(event.id.as_deref(), Some("99"));
        let parsed: serde_json::Value = serde_json::from_str(&event.data).unwrap();
        assert_eq!(parsed["c"], "😀");
    }

    #[test]
    fn take_sse_block_empty_keepalive_between_events() {
        let mut buffer = "\n\ndata: {\"ok\":true}\n\n".to_string();
        assert_eq!(take_sse_block(&mut buffer), Some(String::new()));
        assert_eq!(
            take_sse_block(&mut buffer),
            Some("data: {\"ok\":true}".to_string())
        );
        assert!(buffer.is_empty());
    }
}
