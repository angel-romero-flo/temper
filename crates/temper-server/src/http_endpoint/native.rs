//! Native HTTP transports admitted through declared OData actions.
//!
//! Route configuration owns admission; transports receive only the resulting
//! snapshots and request bytes. The original authenticated context enters the
//! normal OData path, including Cedar, verification, input contracts and IOA.
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use temper_authz::AuthenticatedRequestContext;

use super::MatchedRoute;
use crate::{response::odata_error, state::ServerState};

/// Configuration stored in the endpoint's spec-declared `NativeConfig` field.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeEndpoint {
    /// Host-installed adapter name; endpoint configuration cannot load code.
    pub transport: String,
    /// Ordered admission actions. Every action must succeed before transport.
    pub actions: Vec<AdmissionAction>,
}

pub use super::admission::AdmissionAction;

/// Per-request transport input; never journaled as entity fields.
pub struct TransportRequest {
    /// Exact HTTP method.
    pub method: Method,
    /// Original path and query.
    pub uri: Uri,
    /// Concrete public prefix selected by the declared endpoint.
    pub public_prefix: String,
    /// Request headers with caller credentials and hop headers removed.
    pub headers: HeaderMap,
    /// Bounded request bytes.
    pub body: Bytes,
    /// Successful IOA action responses keyed by admission name.
    pub admitted: BTreeMap<String, Value>,
}

/// I/O-only implementation, installed by the host and selected by a route spec.
#[async_trait::async_trait]
pub trait HttpTransport: Send + Sync {
    /// Execute the admitted HTTP exchange without reimplementing domain policy.
    async fn send(&self, request: TransportRequest) -> Result<Response, String>;
}

/// Named native transports. Registration grants no route or caller permissions.
#[derive(Default)]
pub struct TransportRegistry(BTreeMap<String, Arc<dyn HttpTransport>>);
impl TransportRegistry {
    /// Register a host-owned transport before serving requests.
    pub fn register(&mut self, name: impl Into<String>, transport: Arc<dyn HttpTransport>) {
        self.0.insert(name.into(), transport);
    }
}

/// Remove connection-scoped headers and credentials before external I/O.
pub fn transport_headers(mut headers: HeaderMap) -> HeaderMap {
    let connection_tokens: Vec<String> = headers
        .get_all("connection")
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(','))
        .map(|h| h.trim().to_owned())
        .collect();
    for name in connection_tokens {
        headers.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "authorization",
        "cookie",
        "host",
        "content-length",
        "forwarded",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-forwarded-for",
        "x-tenant-id",
        "x-agent-id",
        "x-agent-type",
    ] {
        headers.remove(name);
    }
    let names: Vec<_> = headers
        .keys()
        .filter(|k| {
            k.as_str().starts_with("x-temper-")
                || k.as_str().starts_with("x-auth-")
                || k.as_str().starts_with("x-auth-request-")
        })
        .cloned()
        .collect();
    for name in names {
        headers.remove(name);
    }
    headers
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch(
    state: &ServerState,
    authenticated: AuthenticatedRequestContext,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
    matched: MatchedRoute,
    config: NativeEndpoint,
) -> Response {
    let timeout = Duration::from_secs(u64::from(matched.route.timeout_secs));
    match tokio::time::timeout(
        timeout,
        exchange(
            state,
            authenticated,
            method,
            uri,
            headers,
            body,
            matched,
            config,
        ),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => odata_error(
            StatusCode::GATEWAY_TIMEOUT,
            "TransportTimeout",
            "HTTP exchange timed out",
        )
        .into_response(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn exchange(
    state: &ServerState,
    authenticated: AuthenticatedRequestContext,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
    matched: MatchedRoute,
    config: NativeEndpoint,
) -> Response {
    let invalid = |message: &str| {
        odata_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InvalidEndpoint",
            message,
        )
        .into_response()
    };
    if config.actions.is_empty() || matched.route.action_bridge.is_some() {
        return invalid(
            "native routes require admission actions and cannot also declare an action bridge",
        );
    }
    let Some(transport) = state.http_transports.0.get(&config.transport) else {
        return invalid("declared native transport is not installed");
    };
    let admitted =
        match super::admission::admit(state, &authenticated, &matched.params, config.actions).await
        {
            Ok(admitted) => admitted,
            Err(response) => return response,
        };
    let body = match to_bytes(body, 8 * 1024 * 1024).await {
        Ok(v) => v,
        Err(_) => {
            return odata_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "RequestTooLarge",
                "request exceeds 8 MiB",
            )
            .into_response();
        }
    };
    // Prefix matching consumes one concrete segment per template segment.
    let segments = matched
        .route
        .path_prefix
        .trim_end_matches('/')
        .split('/')
        .count();
    let public_prefix = uri
        .path()
        .split('/')
        .take(segments)
        .collect::<Vec<_>>()
        .join("/");
    match transport
        .send(TransportRequest {
            method,
            uri,
            public_prefix,
            headers: transport_headers(headers),
            body,
            admitted,
        })
        .await
    {
        Ok(response) => response,
        Err(_) => odata_error(
            StatusCode::BAD_GATEWAY,
            "TransportFailed",
            "upstream transport failed",
        )
        .into_response(),
    }
}

#[cfg(test)]
mod unit_tests {
    use super::super::admission::resolve;
    use super::*;
    #[test]
    fn captures_are_decoded_as_data() {
        let captures = BTreeMap::from([("id".into(), "a%27%2Fb".into())]);
        assert_eq!(resolve("{id}", &captures).unwrap(), "a'/b");
        assert!(resolve("{missing}", &captures).is_err());
        assert!(resolve("prefix-{id}", &captures).is_err());
    }
    #[test]
    fn preserves_odata_headers_but_strips_authority_and_hop_headers() {
        let mut headers = HeaderMap::new();
        for (k, v) in [
            ("authorization", "secret"),
            ("cookie", "secret"),
            ("connection", "x-hop"),
            ("x-hop", "hidden"),
            ("if-match", "etag"),
            ("prefer", "return=minimal"),
            ("odata-version", "4.0"),
            ("x-tenant-id", "other"),
            ("x-temper-principal-kind", "admin"),
        ] {
            headers.insert(k, v.parse().unwrap());
        }
        let result = transport_headers(headers);
        assert_eq!(result.len(), 3);
        assert_eq!(result["if-match"], "etag");
        assert_eq!(result["prefer"], "return=minimal");
        assert_eq!(result["odata-version"], "4.0");
    }
}

#[cfg(test)]
mod tests;
