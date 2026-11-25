//! Signing in to PSN for the account id a registration needs.

use std::fmt::Write as _;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;

use crate::Error;

const LOOKUP: &str = "https://psn.flipscreen.games/search.php";
const SIGN_IN: &str = "https://auth.api.sonyentertainmentnetwork.com/2.0/oauth/authorize?service_entity=urn:service-entity:psn&response_type=code&client_id={CLIENT_ID}&redirect_uri={REDIRECT}&scope={scope}&request_locale=en_US&ui=pr&service_logo=ps&layout_type=popup&smcid=remoteplay&prompt=always&PlatformPrivacyWs1=minimal&";
/// Sony's own account service. The client is the one the console's Remote
/// Play app uses, so signing in here is the same grant a remote play client
/// always asks for.
const CLIENT_ID: &str = "ba495a24-818c-472b-b12d-ff231c1b5745";
/// The Remote Play client ships this value, so it identifies the app.
const CLIENT_SECRET: &str = "mvaiZkRsAsI1IBkY";
const TOKENS: &str = "https://auth.api.sonyentertainmentnetwork.com/2.0/oauth/token";
pub(crate) const REDIRECT: &str = "https://remoteplay.dl.playstation.net/remoteplay/redirect";
const SCOPE: &str = "psn:clientapp referenceDataService:countryConfig.read pushNotification:webSocket.desktop.connect sessionManager:remotePlaySession.system.update";
const TIMEOUT: Duration = Duration::from_secs(10);

/// Where a browser signs in to PSN and ends on a link that carries a code.
pub(crate) fn sign_in_url() -> String {
    SIGN_IN
        .replace("{CLIENT_ID}", CLIENT_ID)
        .replace("{REDIRECT}", REDIRECT)
        .replace("{scope}", &encode(SCOPE))
}

/// The account id in the link PSN sent the browser to after the sign-in: the
/// numeric user id packed little-endian, the form the console compares.
pub(crate) fn account_id_from_redirect(link: &str) -> Result<[u8; 8], Error> {
    let Some(code) = code(link) else {
        return Err(psn(
            "that link carries no code: copy the whole address the browser ended on",
        ));
    };
    let agent = agent();
    let authorization = format!(
        "Basic {}",
        BASE64.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"))
    );
    let token: Value = agent
        .post(TOKENS)
        .header("Authorization", &authorization)
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code),
            ("scope", SCOPE),
            ("redirect_uri", REDIRECT),
        ])
        .map_err(psn)?
        .body_mut()
        .read_json()
        .map_err(psn)?;
    let token = token
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| psn("PSN returned no access token: sign in again for a fresh link"))?;
    let account: Value = agent
        .get(format!("{TOKENS}/{token}"))
        .header("Authorization", &authorization)
        .call()
        .map_err(psn)?
        .body_mut()
        .read_json()
        .map_err(psn)?;
    pack(account.get("user_id").unwrap_or(&Value::Null))
}

/// The account id of the PSN account with that public online id, through the
/// public lookup chiaki-ng offers beside the sign-in.
pub(crate) fn account_id_of(username: &str) -> Result<[u8; 8], Error> {
    let username = username.trim();
    if username.is_empty() {
        return Err(psn("the online id is empty"));
    }
    let found: Value = agent()
        .get(LOOKUP)
        .query("username", username)
        .call()
        .map_err(psn)?
        .body_mut()
        .read_json()
        .map_err(psn)?;
    found
        .get("encoded_id")
        .and_then(Value::as_str)
        .and_then(decode_account_id)
        .ok_or_else(|| {
            psn(found
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("no PSN account has that online id"))
        })
}

fn code(link: &str) -> Option<&str> {
    link.split("code=")
        .nth(1)?
        .split('&')
        .next()
        .filter(|code| !code.is_empty())
}

/// An account id in the base64 form PSN lookups and the command line give it.
pub(crate) fn decode_account_id(text: &str) -> Option<[u8; 8]> {
    BASE64
        .decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
}

fn pack(user_id: &Value) -> Result<[u8; 8], Error> {
    user_id
        .as_str()
        .and_then(|id| id.parse::<u64>().ok())
        .map(u64::to_le_bytes)
        .ok_or_else(|| psn("PSN returned no account id"))
}

/// `text` as a query value: everything but letters, digits and `.-_~` as `%XX`.
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent()
}

fn psn(message: impl std::fmt::Display) -> Error {
    Error::Psn(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::{CLIENT_ID, REDIRECT, account_id_of, code, encode, pack, sign_in_url};
    use serde_json::json;

    #[test]
    fn the_sign_in_link_names_the_remote_play_client() {
        let url = sign_in_url();
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(url.contains(&format!("redirect_uri={REDIRECT}")));
        assert!(url.contains("scope=psn%3Aclientapp%20referenceDataService"));
    }

    #[test]
    fn the_code_comes_out_of_the_link_the_browser_lands_on() {
        assert_eq!(
            code("https://remoteplay.dl.playstation.net/remoteplay/redirect?code=abcd1234"),
            Some("abcd1234")
        );
        assert_eq!(code("https://x/redirect?code=ab&state=s"), Some("ab"));
        assert_eq!(code("https://x/redirect?state=code=1"), Some("1"));
        assert_eq!(code("https://x/redirect?code=&x=1"), None);
        assert_eq!(code("https://x/redirect"), None);
    }

    #[test]
    fn a_user_id_packs_little_endian_and_an_empty_one_is_refused() {
        assert_eq!(pack(&json!("1")).ok(), Some([1, 0, 0, 0, 0, 0, 0, 0]));
        assert!(pack(&json!("")).is_err());
        assert!(pack(&json!(null)).is_err());
    }

    #[test]
    fn query_values_keep_what_a_url_allows() {
        assert_eq!(encode("a b:c/dé"), "a%20b%3Ac%2Fd%C3%A9");
        assert!(account_id_of("  ").is_err());
    }
}
