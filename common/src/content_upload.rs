//! Upload large redacted content before forwarding its log record. The journal
//! retains the original record until every chunk and the final log are accepted.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{send_json_with_agent, PostFailure, PostResponse};

const INLINE_BYTES: usize = 256 * 1024;
const CHUNK_BYTES: usize = 128 * 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;

struct ContentUpload {
    record: Value,
    text: String,
    object_id: String,
}

fn prepare(body: &str, record_id: &str) -> Option<ContentUpload> {
    if body.len() <= INLINE_BYTES {
        return None;
    }
    let mut record: Value = serde_json::from_str(body).ok()?;
    let captured = record.get_mut("captured_content")?.as_object_mut()?;
    if captured.get("capture_status")?.as_str()? != "complete" {
        return None;
    }
    let text = captured.get("text")?.as_str()?.to_string();
    if text.len() <= INLINE_BYTES {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(record_id.as_bytes());
    hasher.update([0]);
    hasher.update(text.as_bytes());
    let object_id = format!("{:x}", hasher.finalize());
    captured.insert("text".into(), Value::Null);
    captured.insert("content_id".into(), Value::String(object_id.clone()));
    Some(ContentUpload {
        record,
        text,
        object_id,
    })
}

fn missing_receipt() -> PostFailure {
    PostFailure {
        message: "Tally content upload returned no matching receipt".into(),
        permanent_record_failure: false,
        retryable: true,
        retry_after: None,
    }
}

fn receipt_matches(response: PostResponse, object_id: &str, status: &[&str]) -> bool {
    response.receipt.as_ref().is_some_and(|receipt| {
        receipt.get("object_id").and_then(Value::as_str) == Some(object_id)
            && receipt
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|value| status.contains(&value))
    })
}

pub(super) fn upload_if_needed(
    agent: &ureq::Agent,
    url: &str,
    api_key: &str,
    body: &str,
    record_id: Option<&str>,
) -> Result<Option<String>, PostFailure> {
    let Some(record_id) = record_id else {
        return Ok(None);
    };
    let Some(upload) = prepare(body, record_id) else {
        return Ok(None);
    };
    let bytes = upload.text.as_bytes();
    if bytes.len() > MAX_BYTES {
        return Err(PostFailure {
            message: "Tally captured content exceeds 8 MiB".into(),
            permanent_record_failure: true,
            retryable: false,
            retry_after: None,
        });
    }
    let chunk_count = bytes.len().div_ceil(CHUNK_BYTES);
    for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let request = json!({"tally_content_upload": {
            "operation": "chunk",
            "record_id": record_id,
            "object_id": upload.object_id,
            "chunk_index": index,
            "chunk_count": chunk_count,
            "upload_bytes": bytes.len(),
            "data_base64": STANDARD.encode(chunk),
        }});
        let idempotency_key = format!("{record_id}:content:{index}");
        let response = send_json_with_agent(
            agent,
            url,
            api_key,
            &request.to_string(),
            Some(&idempotency_key),
            Some(record_id),
        )?;
        if !receipt_matches(response, &upload.object_id, &["stored", "ready"]) {
            return Err(missing_receipt());
        }
    }
    let complete = json!({"tally_content_upload": {
        "operation": "complete", "record_id": record_id, "object_id": upload.object_id,
    }});
    let idempotency_key = format!("{record_id}:content:complete");
    let response = send_json_with_agent(
        agent,
        url,
        api_key,
        &complete.to_string(),
        Some(&idempotency_key),
        Some(record_id),
    )?;
    if !receipt_matches(response, &upload.object_id, &["ready"]) {
        return Err(missing_receipt());
    }
    Ok(Some(upload.record.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use tiny_http::{Response, Server};

    #[test]
    fn large_record_gets_a_stable_reference_without_losing_original_content() {
        let body = json!({"record_type": "TURN_END", "captured_content": {
            "capture_status": "complete", "text": "x".repeat(INLINE_BYTES + 1)
        }})
        .to_string();
        let prepared = prepare(&body, "rec-1").unwrap();
        assert_eq!(prepared.text.len(), INLINE_BYTES + 1);
        assert!(prepared.record["captured_content"]["text"].is_null());
        assert_eq!(
            prepared.record["captured_content"]["content_id"],
            prepared.object_id
        );
        assert_eq!(
            prepare(&body, "rec-1").unwrap().object_id,
            prepared.object_id
        );
        assert_ne!(
            prepare(&body, "rec-2").unwrap().object_id,
            prepared.object_id
        );
    }

    #[test]
    fn large_record_is_uploaded_before_the_log() {
        let server = Server::http(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/v1/tally/logs", server.server_addr());
        let listener = thread::spawn(move || {
            let mut received = Vec::new();
            for _ in 0..5 {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let value: Value = serde_json::from_str(&body).unwrap();
                let upload = value.get("tally_content_upload");
                let receipt = match upload {
                    Some(upload) => json!({
                        "status": if upload["operation"] == "complete" { "ready" } else { "stored" },
                        "object_id": upload["object_id"],
                    }),
                    None => json!({"status": "accepted"}),
                };
                request
                    .respond(Response::from_string(receipt.to_string()))
                    .unwrap();
                received.push(value);
            }
            received
        });
        let text = "x".repeat(INLINE_BYTES + 1);
        let body = json!({"record_type": "TURN_END", "captured_content": {
            "capture_status": "complete", "kind": "agent.output", "text": text,
        }})
        .to_string();
        crate::post_json_with_agent(&crate::http_agent(), &url, "test-key", &body, Some("rec-1"))
            .unwrap_or_else(|error| panic!("{}", error.message));
        let received = listener.join().unwrap();
        let mut uploaded = Vec::new();
        for (index, record) in received[..3].iter().enumerate() {
            let chunk = &record["tally_content_upload"];
            assert_eq!(chunk["record_id"], "rec-1");
            assert_eq!(chunk["chunk_index"], index);
            uploaded.extend(
                STANDARD
                    .decode(chunk["data_base64"].as_str().unwrap())
                    .unwrap(),
            );
        }
        assert_eq!(uploaded, text.as_bytes());
        assert_eq!(received[3]["tally_content_upload"]["operation"], "complete");
        assert_eq!(received[3]["tally_content_upload"]["record_id"], "rec-1");
        assert!(received[4]["captured_content"]["text"].is_null());
        assert!(received[4]["captured_content"]["content_id"].is_string());
    }

    #[test]
    fn gateway_wrapped_upload_error_keeps_the_record_pending() {
        let server = Server::http(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/v1/tally/logs", server.server_addr());
        let listener = thread::spawn(move || {
            let mut request = server.recv().unwrap();
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let has_record_id_header = request.headers().iter().any(|header| {
                header.field.equiv("X-Tally-Record-Id") && header.value.as_str() == "rec-1"
            });
            request
                .respond(Response::from_string(
                    json!({"status_code": 400, "message": "invalid Tally record id"}).to_string(),
                ))
                .unwrap();
            (
                serde_json::from_str::<Value>(&body).unwrap(),
                has_record_id_header,
            )
        });
        let text = "x".repeat(INLINE_BYTES + 1);
        let body = json!({"record_type": "TURN_END", "captured_content": {
            "capture_status": "complete", "text": text,
        }})
        .to_string();
        let error = match crate::post_json_with_agent(
            &crate::http_agent(),
            &url,
            "test-key",
            &body,
            Some("rec-1"),
        ) {
            Ok(_) => panic!("gateway error was accepted"),
            Err(error) => error,
        };
        assert!(!error.permanent_record_failure);
        assert!(!error.retryable);
        let (upload, has_record_id_header) = listener.join().unwrap();
        assert!(has_record_id_header);
        assert_eq!(upload["tally_content_upload"]["operation"], "chunk");
    }
}
