//! Worker state management.

use worker::*;

use crate::database::Database;
use crate::error::{WorkerError, WorkerResult};
use crate::storage::R2Backend;

/// Global worker state extracted from the environment.
pub struct WorkerState {
    /// R2 storage backend for NARs and chunks.
    pub storage: R2Backend,

    /// Database backend (Turso or D1).
    pub database: Database,

    /// JWT signing configuration.
    pub jwt_config: JwtConfig,
}

/// JWT configuration for token validation.
pub struct JwtConfig {
    /// HS256 secret (if using symmetric signing).
    pub hs256_secret: Option<Vec<u8>>,

    /// RS256 public key (if using asymmetric signing).
    pub rs256_pubkey: Option<String>,

    /// Optional bound issuer for token validation.
    pub bound_issuer: Option<String>,

    /// Optional bound audiences for token validation.
    pub bound_audiences: Option<Vec<String>>,
}

impl WorkerState {
    /// Create a new WorkerState from the Cloudflare environment.
    pub fn from_env(env: &Env) -> WorkerResult<Self> {
        // Get R2 bucket
        let bucket = env
            .bucket("CACHE_BUCKET")
            .map_err(|e| WorkerError::Configuration(format!("Missing CACHE_BUCKET: {}", e)))?;

        let storage = R2Backend::new(bucket);

        // Get database configuration
        let database = Database::from_env(env)?;

        // Get JWT configuration
        let jwt_config = JwtConfig::from_env(env)?;

        Ok(Self {
            storage,
            database,
            jwt_config,
        })
    }
}

impl JwtConfig {
    /// Create JWT config from environment variables/secrets.
    pub fn from_env(env: &Env) -> WorkerResult<Self> {
        // Try to get HS256 secret first
        let hs256_secret = env.secret("JWT_HS256_SECRET_BASE64").ok().and_then(|s| {
            use base64::{engine::general_purpose::STANDARD, Engine};
            STANDARD.decode(s.to_string()).ok()
        });

        // Try RS256 public key
        let rs256_pubkey = env.secret("JWT_RS256_PUBKEY_BASE64").ok().and_then(|s| {
            use base64::{engine::general_purpose::STANDARD, Engine};
            STANDARD
                .decode(s.to_string())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
        });

        // Bound issuer (optional)
        let bound_issuer = env.var("JWT_BOUND_ISSUER").ok().map(|v| v.to_string());

        // Bound audiences (optional, comma-separated)
        let bound_audiences = env.var("JWT_BOUND_AUDIENCES").ok().map(|v| {
            v.to_string()
                .split(',')
                .map(|s| s.trim().to_string())
                .collect()
        });

        // At least one signing method must be configured
        if hs256_secret.is_none() && rs256_pubkey.is_none() {
            return Err(WorkerError::Configuration(
                "No JWT signing configuration found. Set JWT_HS256_SECRET_BASE64 or JWT_RS256_PUBKEY_BASE64".to_string(),
            ));
        }

        Ok(Self {
            hs256_secret,
            rs256_pubkey,
            bound_issuer,
            bound_audiences,
        })
    }
}

/// Per-request state with authentication info.
pub struct RequestState {
    /// Authenticated token (if any).
    pub token: Option<attic_token::Token>,
}

impl RequestState {
    /// Extract request state from a Worker request.
    ///
    /// Validates the bearer token's signature and, for admin-issued tokens
    /// (those carrying a `jti`), rejects the request if that token has been
    /// revoked. A revocation-check failure fails open (allows) so a database
    /// hiccup does not take down pulls.
    pub async fn from_request(req: &Request, state: &WorkerState) -> WorkerResult<Self> {
        let token = extract_token(req, &state.jwt_config)?;

        if let Some(ref t) = token {
            if let Some(jti) = t.jwt_id() {
                match state.database.is_token_revoked(jti).await {
                    Ok(true) => {
                        return Err(WorkerError::Authentication(
                            "Token has been revoked".to_string(),
                        ))
                    }
                    Ok(false) => {}
                    Err(e) => console_log!("revocation check failed for jti {}: {}", jti, e),
                }
            }
        }

        Ok(Self { token })
    }
}

/// Extract and validate JWT token from request headers.
fn extract_token(
    req: &Request,
    jwt_config: &JwtConfig,
) -> WorkerResult<Option<attic_token::Token>> {
    let headers = req.headers();

    // Try Authorization header first
    let token_str = if let Some(auth) = headers.get("Authorization").ok().flatten() {
        attic_token::util::parse_authorization_header(&auth)
    } else {
        None
    };

    let token_str = match token_str {
        Some(t) => t,
        None => return Ok(None),
    };

    // Build signature type from config
    let signature_type = if let Some(ref secret) = jwt_config.hs256_secret {
        attic_token::SignatureType::HS256(attic_token::HS256Key::from_bytes(secret))
    } else if let Some(ref pubkey) = jwt_config.rs256_pubkey {
        let key = attic_token::RS256PublicKey::from_pem(pubkey)
            .map_err(|e| WorkerError::Authentication(format!("Invalid RS256 key: {}", e)))?;
        attic_token::SignatureType::RS256PubkeyOnly(key)
    } else {
        return Err(WorkerError::Configuration(
            "No JWT signing configuration".to_string(),
        ));
    };

    // Convert bound audiences to HashSet
    let bound_audiences = jwt_config
        .bound_audiences
        .as_ref()
        .map(|v| v.iter().cloned().collect());

    // Validate token
    let token = attic_token::Token::from_jwt(
        &token_str,
        &signature_type,
        &jwt_config.bound_issuer,
        &bound_audiences,
    )
    .map_err(|e| WorkerError::Authentication(format!("Invalid token: {}", e)))?;

    Ok(Some(token))
}
