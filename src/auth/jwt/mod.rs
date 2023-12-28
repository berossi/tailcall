mod validation;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use headers::authorization::Bearer;
use headers::Authorization;
use headers::HeaderMapExt;
use jwtk::jwk::JwkSet;
use jwtk::jwk::JwkSetVerifier;
use jwtk::jwk::RemoteJwksVerifier;
use jwtk::HeaderAndClaims;
use serde::Deserialize;
use serde::Serialize;

use crate::helpers::config_path::config_path;
use crate::http::RequestContext;
use crate::valid::Valid;

use self::validation::validate_aud;
use self::validation::validate_iss;

use super::base::{AuthError, AuthProvider};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub enum JwtProviderJwksOptions {
  File(PathBuf),
  Remote { url: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JwtProviderOptions {
  issuer: Option<String>,
  #[serde(default)]
  audiences: HashSet<String>,
  jwks: JwtProviderJwksOptions,
}

// only used in tests and uses mocked implementation
#[cfg(test)]
impl Default for JwtProviderOptions {
  fn default() -> Self {
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut test_file = root_dir.join(file!());

    test_file.pop();
    test_file.push("tests");
    test_file.push("jwks.json");

    let jwks = JwtProviderJwksOptions::File(test_file);

    Self { issuer: Default::default(), audiences: Default::default(), jwks }
  }
}

enum JwksVerifier {
  Local(JwkSetVerifier),
  Remote(RemoteJwksVerifier),
}

impl TryFrom<&JwtProviderJwksOptions> for JwksVerifier {
  type Error = anyhow::Error;

  fn try_from(value: &JwtProviderJwksOptions) -> anyhow::Result<Self> {
    match value {
      JwtProviderJwksOptions::File(path) => {
        let file = fs::read_to_string(config_path(&path)?)?;
        let jwks: JwkSet = serde_json::from_str(&file)?;

        Ok(Self::Local(jwks.verifier()))
      }
      JwtProviderJwksOptions::Remote { url } => Ok(Self::Remote(RemoteJwksVerifier::new(
        url.to_owned(),
        // TODO: set client?
        None,
        // TODO: set duration from config
        Duration::from_secs(5 * 3600),
      ))),
    }
  }
}

impl JwksVerifier {
  async fn verify(&self, token: &str) -> jwtk::Result<HeaderAndClaims<()>> {
    match self {
      JwksVerifier::Local(verifier) => verifier.verify(token),
      JwksVerifier::Remote(verifier) => verifier.verify(token).await,
    }
  }
}

pub struct JwtProvider {
  options: JwtProviderOptions,
  verifier: JwksVerifier,
}

impl TryFrom<JwtProviderOptions> for JwtProvider {
  type Error = anyhow::Error;
  fn try_from(options: JwtProviderOptions) -> anyhow::Result<Self> {
    let verifier = JwksVerifier::try_from(&options.jwks)?;

    Ok(Self { options, verifier })
  }
}

#[async_trait::async_trait]
impl AuthProvider for JwtProvider {
  async fn validate(&self, request: &RequestContext) -> Valid<(), AuthError> {
    let token = self.resolve_token(request);

    let Some(token) = token else {
      return Valid::fail(AuthError::Missing);
    };

    self.validate_token(&token).await
  }
}

impl JwtProvider {
  fn resolve_token(&self, request: &RequestContext) -> Option<String> {
    let value = request.req_headers.typed_get::<Authorization<Bearer>>();

    value.map(|token| token.token().to_owned())
  }

  async fn validate_token(&self, token: &str) -> Valid<(), AuthError> {
    let c = self.verifier.verify(&token).await.unwrap();

    self.validate_claims(&c)
  }

  fn validate_claims(&self, parsed: &HeaderAndClaims<()>) -> Valid<(), AuthError> {
    let claims = parsed.claims();

    if !validate_iss(&self.options, claims) || !validate_aud(&self.options, claims) {
      return Valid::fail(AuthError::ValidationFailed);
    }

    Valid::succeed(())
  }
}

#[cfg(test)]
mod tests {
  use anyhow::Result;
  use serde_json::json;

  use crate::valid::ValidationError;

  use super::*;

  // token with issuer = "me" and audience = ["them"]
  // token is valid for 10 years. It it expired, update it =)
  // to parse the token and see its content use https://jwt.io
  const TEST_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6Ikk0OHFNSnA1NjZTU0tRb2dZWFl0SEJvOXE2WmNFS0hpeE5QZU5veFYxYzgifQ.eyJleHAiOjIwMTkwNTY0NDEuMCwiaXNzIjoibWUiLCJzdWIiOiJ5b3UiLCJhdWQiOlsidGhlbSJdfQ.cU-hJgVGWxK3-IBggYBChhf3FzibBKjuDLtq2urJ99FVXIGZls0VMXjyNW7yHhLLuif_9t2N5UIUIq-hwXVv7rrGRPCGrlqKU0jsUH251Spy7_ppG5_B2LsG3cBJcwkD4AVz8qjT3AaE_vYZ4WnH-CQ-F5Vm7wiYZgbdyU8xgKoH85KAxaCdJJlYOi8mApE9_zcdmTNJrTNd9sp7PX3lXSUu9AWlrZkyO-HhVbXFunVtfduDuTeVXxP8iw1wt6171CFbPmQJU_b3xCornzyFKmhSc36yvlDfoPPclWmWeyOfFEp9lVhQm0WhfDK7GiuRtaOxD-tOvpTjpcoZBeJb7bSg2OsneyeM_33a0WoPmjHw8WIxbroJz_PrfE72_TzbcTSDttKAv_e75PE48Vvx0661miFv4Gq8RBzMl2G3pQMEVCOm83v7BpodfN_YVJcqZJjVHMA70TZQ4K3L4_i9sIK9jJFfwEDVM7nsDnUu96n4vKs1fVvAuieCIPAJrfNOUMy7TwLvhnhUARsKnzmtNNrJuDhhBx-X93AHcG3micXgnqkFdKn6-ZUZ63I2KEdmjwKmLTRrv4n4eZKrRN-OrHPI4gLxJUhmyPAHzZrikMVBcDYfALqyki5SeKkwd4v0JAm87QzR4YwMdKErr0Xa5JrZqHGe2TZgVO4hIc-KrPw";

  #[test]
  fn jwt_options_parse() -> Result<()> {
    let options: JwtProviderOptions = serde_json::from_value(json!({
      "jwks": {
        "file": "tests/server/config/jwks.json"
      }
    }))?;

    assert!(matches!(options.jwks, JwtProviderJwksOptions::File(_)));

    let options: JwtProviderOptions = serde_json::from_value(json!({
      "jwks": {
        "remote": {
          "url": "http://localhost:3000"
        }
      }
    }))?;

    assert!(matches!(options.jwks, JwtProviderJwksOptions::Remote { .. }));

    Ok(())
  }

  #[tokio::test]
  async fn validate_token_iss() -> Result<()> {
    let jwt_options = JwtProviderOptions::default();
    let jwt_provider = JwtProvider::try_from(jwt_options)?;

    let valid = jwt_provider.validate_token(TEST_TOKEN).await;

    assert!(valid.is_succeed());

    let mut jwt_options = JwtProviderOptions::default();
    jwt_options.issuer = Some("me".to_owned());
    let jwt_provider = JwtProvider::try_from(jwt_options)?;

    let valid = jwt_provider.validate_token(TEST_TOKEN).await;

    assert!(valid.is_succeed());

    let mut jwt_options = JwtProviderOptions::default();
    jwt_options.issuer = Some("another".to_owned());
    let jwt_provider = JwtProvider::try_from(jwt_options)?;

    let error = jwt_provider.validate_token(TEST_TOKEN).await.to_result().err();

    assert_eq!(error, Some(ValidationError::new(AuthError::ValidationFailed)));

    Ok(())
  }

  #[tokio::test]
  async fn validate_token_aud() -> Result<()> {
    let jwt_options = JwtProviderOptions::default();
    let jwt_provider = JwtProvider::try_from(jwt_options)?;

    let valid = jwt_provider.validate_token(TEST_TOKEN).await;

    assert!(valid.is_succeed());

    let mut jwt_options = JwtProviderOptions::default();
    jwt_options.audiences = HashSet::from_iter(["them".to_string()]);
    let jwt_provider = JwtProvider::try_from(jwt_options)?;

    let valid = jwt_provider.validate_token(TEST_TOKEN).await;

    assert!(valid.is_succeed());

    let mut jwt_options = JwtProviderOptions::default();
    jwt_options.audiences = HashSet::from_iter(["anothem".to_string()]);
    let jwt_provider = JwtProvider::try_from(jwt_options)?;

    let error = jwt_provider.validate_token(TEST_TOKEN).await.to_result().err();

    assert_eq!(error, Some(ValidationError::new(AuthError::ValidationFailed)));
    Ok(())
  }
}
