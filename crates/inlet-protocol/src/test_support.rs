//! Helpers for the tests that run against the private test instance, behind the same
//! cargo feature as [`crate::test_ca`] (never in a release build): a throwaway account
//! with a random password kept in memory, and the six-digit codes an authenticator app
//! would show (RFC 6238, SHA-1), so a test can turn two-factor on and use it.
//!
//! Nothing here reads a credential from disk. The accounts are made up by the test,
//! on an instance whose registration is open, and deleted at the end.

use rand::RngCore;
use serde_json::{json, Value};

/// `bytes` random bytes as hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// A freshly registered throwaway account.
pub struct Throwaway {
    /// The account name (`<prefix>_<random>`).
    pub username: String,
    /// A random password, in memory only.
    pub password: String,
    /// The session token that registering returned.
    pub token: String,
    /// The account ID.
    pub user_id: String,
}

/// One JSON request to the instance's API with the test CA trusted. `token` is sent as
/// the `Authorization` header when given.
pub async fn call(
    http: &reqwest::Client,
    api_base: &str,
    method: reqwest::Method,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (u16, Value) {
    let mut req = http
        .request(method, format!("{api_base}{path}"))
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if let Some(token) = token {
        req = req.header("Authorization", token);
    }
    let resp = req.send().await.expect("the test instance answers");
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// An HTTP client that trusts the test CA.
pub fn http() -> reqwest::Client {
    crate::test_ca::add_to_http(reqwest::Client::builder())
        .build()
        .expect("http client")
}

impl Throwaway {
    /// Registers `<prefix>_<random>` with a random password (the test instance takes a
    /// username and no e-mail, and has no CAPTCHA). `register_path` is the route of the
    /// registration; the test names it, so this file holds no route (the API snapshot
    /// tool lists the routes of the library's sources only).
    pub async fn register(
        http: &reqwest::Client,
        api_base: &str,
        register_path: &str,
        prefix: &str,
    ) -> Throwaway {
        let username = format!("{prefix}_{}", random_hex(6));
        let password = random_hex(16);
        let (status, v) = call(
            http,
            api_base,
            reqwest::Method::POST,
            register_path,
            None,
            json!({"username": username, "password": password, "global_name": username,
                   "date_of_birth": "1990-01-01", "consent": true}),
        )
        .await;
        assert_eq!(status, 200, "registration: {} {}", v["code"], v["message"]);
        Throwaway {
            username,
            password,
            token: v["token"].as_str().expect("a token").to_owned(),
            user_id: v["user_id"].as_str().expect("a user id").to_owned(),
        }
    }

    /// Deletes the account at once (the account is left behind if the instance refuses).
    /// `proof` is the body for sudo mode: `{"password": ..}` or `{"mfa_method": "totp",
    /// "mfa_code": ..}`.
    pub async fn delete(
        &self,
        http: &reqwest::Client,
        api_base: &str,
        delete_path: &str,
        proof: Value,
    ) -> u16 {
        let mut body = proof;
        body["immediate"] = json!(true);
        call(
            http,
            api_base,
            reqwest::Method::POST,
            delete_path,
            Some(&self.token),
            body,
        )
        .await
        .0
    }
}

/// The current 30-second step number.
pub fn totp_step_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 30
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, b) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(key);
    let pad = |x: u8| -> Vec<u8> { block.iter().map(|b| b ^ x).collect() };
    let mut inner = pad(0x36);
    inner.extend_from_slice(msg);
    let mut outer = pad(0x5c);
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// The six-digit TOTP code of `secret` for 30-second step number `step`.
pub fn totp(secret: &[u8], step: u64) -> String {
    let mac = hmac_sha1(secret, &step.to_be_bytes());
    let o = (mac[19] & 0xf) as usize;
    let n = u32::from_be_bytes([mac[o] & 0x7f, mac[o + 1], mac[o + 2], mac[o + 3]]);
    format!("{:06}", n % 1_000_000)
}

/// RFC 4648 base32 without padding (the form an authenticator secret is shown in).
pub fn base32(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut out, mut buf, mut bits) = (String::new(), 0u32, 0);
    for b in data {
        buf = (buf << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            out.push(A[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
        buf &= 0xff;
    }
    if bits > 0 {
        out.push(A[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_totp_helpers_match_the_rfc_test_vectors() {
        // RFC 6238 appendix B (SHA-1, secret "12345678901234567890"): T = 59 s is step 1.
        assert_eq!(totp(b"12345678901234567890", 1), "287082");
        assert_eq!(totp(b"12345678901234567890", 37_037_036), "081804");
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }
}
