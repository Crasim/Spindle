use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use reqwest::{Client, Url};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    time::{Duration, Instant},
};
const ISSUER: &str = "https://auth.openai.com";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub const API: &str = "https://api.openai.com/v1";
const GRANT: &str = "chatgpt.tokens.use.direct";

#[derive(Clone, Serialize, Deserialize)]
pub struct Registration {
    pub client_id: String,
    pub subject: String,
    pub label: String,
    pub vault: String,
}
#[derive(Deserialize, Serialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    expires: i64,
    scope: String,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    scope: Option<String>,
    id_token: Option<String>,
}

fn dir() -> Result<PathBuf> {
    let p = crate::state::data_dir()?.join("Spindle").join("accounts");
    fs::create_dir_all(&p)?;
    Ok(p)
}
fn file(reg: &Registration) -> Result<PathBuf> {
    ensure!(
        !reg.vault.is_empty()
            && reg
                .vault
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "Invalid account reference"
    );
    Ok(dir()?.join(format!("{}.bin", reg.vault)))
}
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0; N];
    SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| anyhow::anyhow!("System random generator unavailable"))?;
    Ok(b)
}
fn host_id() -> Result<String> {
    let folder = dir()?;
    let path = folder.join("host-id");
    if !path.exists() {
        let mut r = random::<16>()?;
        r[6] = (r[6] & 15) | 64;
        r[8] = (r[8] & 63) | 128;
        let hex = r.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut f = tempfile::NamedTempFile::new_in(folder)?;
        write!(
            f,
            "urn:uuid:{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )?;
        f.as_file().sync_all()?;
        if let Err(e) = f.persist_noclobber(&path) {
            if !path.exists() {
                return Err(e.into());
            }
        }
    }
    Ok(fs::read_to_string(path)?)
}
fn save(reg: &Registration, t: &Tokens) -> Result<()> {
    save_at(&file(reg)?, t)
}
fn save_at(path: &std::path::Path, t: &Tokens) -> Result<()> {
    let bytes = protect(&serde_json::to_vec(t)?, true)?;
    let mut f =
        tempfile::NamedTempFile::new_in(path.parent().context("Missing credential directory")?)?;
    f.write_all(&bytes)?;
    f.as_file().sync_all()?;
    f.persist(path)?;
    Ok(())
}
fn load(reg: &Registration) -> Result<Tokens> {
    load_at(&file(reg)?)
}
fn load_at(path: &std::path::Path) -> Result<Tokens> {
    let bytes = protect(
        &fs::read(path).context("Sign in to this ChatGPT connection")?,
        false,
    )?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn protect(data: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    #[cfg(windows)]
    return crate::vault::local(data, encrypt);
    #[cfg(target_os = "android")]
    return crate::android::protect_provider_credential(data, encrypt);
}

#[cfg(target_os = "android")]
pub(crate) fn android_key_path(target: &str) -> Result<PathBuf> {
    let digest = digest::digest(&digest::SHA256, target.as_bytes());
    Ok(dir()?.join(format!(
        "key-{}.bin",
        crate::sync::core::hex(digest.as_ref())
    )))
}

#[cfg(target_os = "android")]
pub(crate) fn store_android_key(target: &str, key: &str) -> Result<()> {
    let path = android_key_path(target)?;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(&protect(key.as_bytes(), true)?)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[cfg(target_os = "android")]
pub(crate) fn android_key(target: &str) -> Result<String> {
    let bytes = fs::read(android_key_path(target)?)?;
    Ok(String::from_utf8(protect(&bytes, false)?)?)
}

fn callback(listener: TcpListener, state: &str) -> Result<(String, Option<String>)> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(240);
    while Instant::now() < deadline {
        let (mut stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(80));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut line = String::new();
        use std::io::BufRead;
        if std::io::BufReader::new((&stream).take(8192))
            .read_line(&mut line)
            .is_err()
        {
            continue;
        }
        let mut parts = line.split_whitespace();
        if parts.next() != Some("GET") {
            continue;
        }
        let Some(path) = parts.next().filter(|p| p.starts_with("/auth/callback?")) else {
            continue;
        };
        let url = Url::parse(&format!("http://127.0.0.1{path}"))?;
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        if query.get("state").map(String::as_str) != Some(state) {
            let _ =
                stream.write_all(b"HTTP/1.0 400 Bad Request\r\n\r\nUnrecognized sign-in attempt.");
            continue;
        }
        let _=stream.write_all(b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\n\r\nReturn to Spindle to see the connection result.");
        ensure!(
            !query.contains_key("error"),
            "ChatGPT authorization was declined"
        );
        return Ok((
            query
                .get("code")
                .context("Authorization code missing")?
                .clone(),
            query.get("client_id").cloned(),
        ));
    }
    bail!("Sign-in timed out. Choose Continue with ChatGPT to try again")
}
async fn exchange(http: &Client, data: &[(&str, &str)]) -> Result<TokenResponse> {
    let response = http
        .post(TOKEN)
        .form(data)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "ChatGPT token exchange returned {}. Reconnect if the grant expired.",
        response.status()
    );
    let token: TokenResponse = response.json().await?;
    ensure!(
        !token.access_token.is_empty() && token.expires_in > 0,
        "Invalid OAuth token response"
    );
    Ok(token)
}
pub async fn sign_in(
    http: &Client,
    vault: String,
    prior: Option<&Registration>,
) -> Result<(Registration, String)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener.local_addr()?.port()
    );
    let state = B64.encode(random::<32>()?);
    let nonce = B64.encode(random::<32>()?);
    let verifier = B64.encode(random::<48>()?);
    let challenge = B64.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));
    let mut url = Url::parse("https://auth.openai.com/api/accounts/authorize")?;
    url.query_pairs_mut().extend_pairs([
        (
            "client_id",
            prior
                .map(|r| r.client_id.as_str())
                .unwrap_or("dynamic_agent_client"),
        ),
        ("ext_agent_host_id", &host_id()?),
        ("response_type", "code"),
        ("redirect_uri", &redirect),
        (
            "scope",
            "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
        ),
        ("resource", API),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge_method", "S256"),
        ("code_challenge", &challenge),
    ]);
    if prior.is_none() {
        url.query_pairs_mut()
            .append_pair("agent_name_hint", "Spindle");
    }
    webbrowser::open(url.as_str()).context("Open system browser for sign-in")?;
    let (code, issued) = tokio::task::spawn_blocking(move || callback(listener, &state)).await??;
    let client_id = match prior {
        Some(r) => {
            ensure!(
                issued.as_ref().is_none_or(|id| id == &r.client_id),
                "Registration changed during sign-in"
            );
            r.client_id.clone()
        }
        None => issued
            .filter(|id| !id.is_empty() && id != "dynamic_agent_client")
            .context("Issued client ID missing")?,
    };
    let token = exchange(
        http,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
            ("resource", API),
        ],
    )
    .await?;
    let id = token.id_token.context("ID token missing")?;
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
    let metadata: Value = http
        .get(format!("{ISSUER}/.well-known/openid-configuration"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(metadata["issuer"] == ISSUER, "Unexpected OpenID issuer");
    let jwks = Url::parse(metadata["jwks_uri"].as_str().context("Missing JWKS URI")?)?;
    ensure!(
        jwks.scheme() == "https" && jwks.host_str() == Some("auth.openai.com"),
        "Unexpected OpenID key authority"
    );
    let keys: JwkSet = http
        .get(jwks)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let header = decode_header(&id)?;
    ensure!(
        header.alg == Algorithm::RS256,
        "Unexpected ID token algorithm"
    );
    let key = keys
        .find(&header.kid.context("Missing signing key ID")?)
        .context("Unknown signing key")?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[&client_id]);
    validation.set_issuer(&[ISSUER]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let claims = decode::<Value>(&id, &DecodingKey::from_jwk(key)?, &validation)?.claims;
    let subject = claims["sub"]
        .as_str()
        .context("Missing subject")?
        .to_owned();
    ensure!(
        claims["nonce"].as_str() == Some(nonce.as_str())
            && prior.is_none_or(|r| r.subject == subject),
        "Account identity did not match this sign-in"
    );
    let scope = token.scope.unwrap_or_default();
    ensure!(
        scope.split_whitespace().any(|s| s == GRANT),
        "ChatGPT plan usage permission was not granted"
    );
    let label = claims["email"]
        .as_str()
        .unwrap_or("ChatGPT account")
        .to_owned();
    let reg = Registration {
        client_id,
        subject,
        label,
        vault,
    };
    let tokens = Tokens {
        access_token: token.access_token,
        refresh_token: token.refresh_token.context("Renewable grant missing")?,
        scope,
        expires: crate::automation::now() + token.expires_in,
    };
    save(&reg, &tokens)?;
    Ok((reg, tokens.access_token))
}

static RENEW: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub async fn access(http: &Client, reg: &Registration) -> Result<String> {
    let _guard = RENEW.lock().await;
    let mut t = load(reg)?;
    if t.expires <= crate::automation::now() + 90 {
        let new = exchange(
            http,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", &reg.client_id),
                ("refresh_token", &t.refresh_token),
                ("resource", API),
            ],
        )
        .await?;
        t.access_token = new.access_token;
        if let Some(r) = new.refresh_token {
            t.refresh_token = r;
        }
        if let Some(s) = new.scope {
            t.scope = s;
        }
        t.expires = crate::automation::now() + new.expires_in;
        save(reg, &t)?;
    }
    ensure!(
        t.scope.split_whitespace().any(|s| s == GRANT),
        "ChatGPT plan permission missing"
    );
    Ok(t.access_token)
}
pub async fn revoke(http: &Client, reg: &Registration) -> Result<()> {
    let _guard = RENEW.lock().await;
    let t = load(reg)?;
    let metadata: Value = http
        .get(format!("{ISSUER}/.well-known/openid-configuration"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let url = Url::parse(
        metadata["revocation_endpoint"]
            .as_str()
            .context("Missing revocation endpoint")?,
    )?;
    ensure!(
        url.scheme() == "https" && url.host_str() == Some("auth.openai.com"),
        "Unexpected revocation authority"
    );
    http.post(url)
        .form(&[
            ("token", &*t.refresh_token),
            ("token_type_hint", "refresh_token"),
            ("client_id", &reg.client_id),
        ])
        .send()
        .await?
        .error_for_status()?;
    fs::remove_file(file(reg)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn windows_account_storage_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.bin");
        let tokens = Tokens {
            access_token: "synthetic-access-value".into(),
            refresh_token: "synthetic-refresh-value".into(),
            expires: 123,
            scope: GRANT.into(),
        };
        save_at(&path, &tokens).unwrap();
        assert!(!String::from_utf8_lossy(&fs::read(&path).unwrap()).contains("synthetic"));
        let result = load_at(&path).unwrap();
        assert_eq!(result.refresh_token, tokens.refresh_token);
        assert_eq!(result.access_token, tokens.access_token);
    }
    #[test]
    fn loopback_callback_matches_pending_sign_in() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let reader = std::thread::spawn(move || callback(listener, "pending-registration"));
        for (state, status) in [("old-attempt", "400"), ("pending-registration", "200")] {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            write!(stream,"GET /auth/callback?state={state}&code=sample&client_id=test-client HTTP/1.0\r\n\r\n").unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.contains(status));
        }
        let (code, id) = reader.join().unwrap().unwrap();
        assert_eq!(code, "sample");
        assert_eq!(id.as_deref(), Some("test-client"));
    }
}
