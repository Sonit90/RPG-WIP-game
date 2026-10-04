//! Google OAuth2 (auth-code flow). Other providers can plug in alongside.

use oauth2::{
    basic::BasicClient, AuthUrl, AuthorizationCode, ClientId, ClientSecret,
    CsrfToken, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use generic_auth_core::AppError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GoogleProfile {
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";

pub fn google_client(cfg: &GoogleConfig) -> Result<BasicClient, AppError> {
    let auth_url = AuthUrl::new(GOOGLE_AUTH_URL.into())
        .map_err(|e| AppError::Auth(format!("auth url: {e}")))?;
    let token_url = TokenUrl::new(GOOGLE_TOKEN_URL.into())
        .map_err(|e| AppError::Auth(format!("token url: {e}")))?;
    let redirect = RedirectUrl::new(cfg.redirect_url.clone())
        .map_err(|e| AppError::Auth(format!("redirect url: {e}")))?;

    Ok(BasicClient::new(
        ClientId::new(cfg.client_id.clone()),
        Some(ClientSecret::new(cfg.client_secret.clone())),
        auth_url,
        Some(token_url),
    )
    .set_redirect_uri(redirect))
}

/// Build an authorize URL plus CSRF state. Caller stores state in a
/// short-lived signed cookie or session and validates it on callback.
pub fn google_authorize_url(client: &BasicClient) -> (url::Url, CsrfToken) {
    client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .url()
}

/// Exchange the auth code for tokens, then fetch the userinfo profile.
pub async fn google_exchange_code(
    client: &BasicClient,
    code: String,
    http: &reqwest::Client,
) -> Result<GoogleProfile, AppError> {
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .map_err(|e| AppError::Auth(format!("token exchange: {e}")))?;

    let resp = http
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(token.access_token().secret())
        .send()
        .await
        .map_err(|e| AppError::Auth(format!("userinfo: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Auth(format!("userinfo status: {e}")))?
        .json::<GoogleProfile>()
        .await
        .map_err(|e| AppError::Auth(format!("userinfo decode: {e}")))?;

    Ok(resp)
}
