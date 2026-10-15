//! Blocking HTTP used by online lyrics and cover lookup.
//! Failures stay local: a timeout or a missing network is a miss, not a crash.

use std::io::Read;
use std::time::Duration;

pub const USER_AGENT: &str = "Chengyin/0.4.0 (https://github.com/wuan1250276415/ClearSound)";

#[derive(Debug)]
pub enum HttpBody {
    Bytes(Vec<u8>),
    /// The server answered, but not with a usable body (404 and other non-2xx).
    Status(u16),
    /// Timeout, DNS, or no connection.
    Offline,
}

pub fn get(url: &str, referer: Option<&str>, limit: u64) -> HttpBody {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout_read(Duration::from_secs(6))
        .redirects(5)
        .build();
    let mut request = agent.get(url).set("User-Agent", USER_AGENT).set("Accept", "*/*");
    if let Some(referer) = referer {
        request = request.set("Referer", referer);
    }
    match request.call() {
        Ok(response) => read_limited(response, limit),
        Err(ureq::Error::Status(code, _)) => HttpBody::Status(code),
        Err(_) => HttpBody::Offline,
    }
}

fn read_limited(response: ureq::Response, limit: u64) -> HttpBody {
    let mut buf = Vec::new();
    match response.into_reader().take(limit).read_to_end(&mut buf) {
        Ok(_) if !buf.is_empty() => HttpBody::Bytes(buf),
        Ok(_) => HttpBody::Status(204),
        Err(_) => HttpBody::Offline,
    }
}

pub fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
