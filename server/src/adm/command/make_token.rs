use anyhow::{Result, anyhow};
use chrono::{Duration as ChronoDuration, Utc};
use clap::Parser;
use humantime::Duration;

use attic::cache::CacheNamePattern;
use attic_server::access::Token;
use attic_server::config::Config;
use attic_token::{decode_token_hs256_secret_base64, SignatureType};
use crate::Opts;

/// Generate a new token.
///
/// For example, to generate a token for Alice with read-write access
/// to any cache starting with `dev-` and read-only access to `prod`,
/// expiring in 2 years:
///
/// $ atticadm make-token --sub "alice" --validity "2y" --pull "dev-*" --push "dev-*" --pull "prod"
///
/// You can also pass the secret directly without a config file:
///
/// $ atticadm make-token --secret-base64 "YOUR_SECRET" --sub "admin" --validity "1y" --pull "*" --push "*"
#[derive(Debug, Parser)]
pub struct MakeToken {
    /// Base64-encoded HS256 secret for signing.
    ///
    /// If provided, the config file is not required.
    #[clap(long, env = "ATTIC_SERVER_TOKEN_HS256_SECRET_BASE64")]
    pub secret_base64: Option<String>,

    /// The subject of the JWT token.
    #[clap(long)]
    sub: String,

    /// The validity period of the JWT token.
    ///
    /// You can use expressions like "2 years", "3 months"
    /// and "1y".
    #[clap(long)]
    validity: Duration,

    /// Dump the claims without signing and encoding it.
    #[clap(long)]
    pub dump_claims: bool,

    /// A cache that the token may pull from.
    ///
    /// The value may contain wildcards. Specify this flag multiple
    /// times to allow multiple patterns.
    #[clap(long = "pull", value_name = "PATTERN")]
    pull_patterns: Vec<CacheNamePattern>,

    /// A cache that the token may push to.
    ///
    /// The value may contain wildcards. Specify this flag multiple
    /// times to allow multiple patterns.
    #[clap(long = "push", value_name = "PATTERN")]
    push_patterns: Vec<CacheNamePattern>,

    /// A cache that the token may delete store paths from.
    ///
    /// The value may contain wildcards. Specify this flag multiple
    /// times to allow multiple patterns.
    #[clap(long = "delete", value_name = "PATTERN")]
    delete_patterns: Vec<CacheNamePattern>,

    /// A cache that the token may create.
    ///
    /// The value may contain wildcards. Specify this flag multiple
    /// times to allow multiple patterns.
    #[clap(long = "create-cache", value_name = "PATTERN")]
    create_cache_patterns: Vec<CacheNamePattern>,

    /// A cache that the token may configure.
    ///
    /// The value may contain wildcards. Specify this flag multiple
    /// times to allow multiple patterns.
    #[clap(long = "configure-cache", value_name = "PATTERN")]
    configure_cache_patterns: Vec<CacheNamePattern>,

    /// A cache that the token may configure retention/quota for.
    ///
    /// The value may contain wildcards. Specify this flag multiple
    /// times to allow multiple patterns.
    #[clap(long = "configure-cache-retention", value_name = "PATTERN")]
    configure_cache_retention_patterns: Vec<CacheNamePattern>,

    /// A cache that the token may destroy.
    ///
    /// The value may contain wildcards. Specify this flag multiple
    /// times to allow multiple patterns.
    #[clap(long = "destroy-cache", value_name = "PATTERN")]
    destroy_cache_patterns: Vec<CacheNamePattern>,
}

macro_rules! grant_permissions {
    ($token:ident, $list:expr, $perm:ident) => {
        for pattern in $list {
            let perm = $token.get_or_insert_permission_mut(pattern.to_owned());
            perm.$perm = true;
        }
    };
}

pub async fn run(config: Option<Config>, opts: Opts) -> Result<()> {
    let sub = opts.command.as_make_token().unwrap();
    let duration = ChronoDuration::from_std(sub.validity.into())?;
    let exp = Utc::now()
        .checked_add_signed(duration)
        .ok_or_else(|| anyhow!("Expiry timestamp overflowed"))?;

    let mut token = Token::new(sub.sub.to_owned(), &exp);

    grant_permissions!(token, &sub.pull_patterns, pull);
    grant_permissions!(token, &sub.push_patterns, push);
    grant_permissions!(token, &sub.delete_patterns, delete);
    grant_permissions!(token, &sub.create_cache_patterns, create_cache);
    grant_permissions!(token, &sub.configure_cache_patterns, configure_cache);
    grant_permissions!(
        token,
        &sub.configure_cache_retention_patterns,
        configure_cache_retention
    );
    grant_permissions!(token, &sub.destroy_cache_patterns, destroy_cache);

    if sub.dump_claims {
        println!("{}", serde_json::to_string(token.opaque_claims())?);
    } else {
        // Determine signature type: prefer direct secret, fall back to config
        let (signature_type, bound_issuer, bound_audiences) =
            if let Some(ref secret) = sub.secret_base64 {
                let key = decode_token_hs256_secret_base64(secret)
                    .map_err(|e| anyhow!("Failed to decode secret: {}", e))?;
                (SignatureType::HS256(key), None, None)
            } else if let Some(config) = config {
                (
                    config.jwt.signing_config.into(),
                    config.jwt.token_bound_issuer,
                    config.jwt.token_bound_audiences,
                )
            } else {
                return Err(anyhow!(
                "Either --secret-base64 or a config file with JWT signing configuration is required"
            ));
            };

        let encoded_token = token.encode(&signature_type, &bound_issuer, &bound_audiences)?;
        println!("{}", encoded_token);
    }

    Ok(())
}
