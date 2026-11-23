//! The MCP server (`docs/specs/access.md`, The MCP server): a tool for each
//! action, served at `/mcp` over Streamable HTTP to the clients that act
//! as a person with an access token of the authorization server.
//! <https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#streamable-http>

mod page;
mod tools;

use std::borrow::Cow;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tracing::{error, info};

use crate::access::Caller;
use crate::access::oauth::store::AccessTokens;
use crate::access::oauth::{self, MCP_PATH};
use crate::access::people::ManagerName;
use crate::api::Sources;

/// The versions of the protocol it serves: those from 2025-06-18 on.
const PROTOCOL_VERSIONS: [ProtocolVersion; 3] = [
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];

/// The MCP server at [`MCP_PATH`], acting as the person whose access token
/// each request bears, under the `manager` setting `manager`. A request
/// without a valid access token gets 401, pointing to `server`'s protected
/// resource metadata. It answers whatever `Host` a request names.
pub fn router<S: Clone + Send + Sync + 'static>(
    sources: Sources,
    server: &oauth::Server,
    manager: Option<ManagerName>,
) -> Router<S> {
    let handler = Handler {
        sources: sources.clone(),
        manager,
    };
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None)
            .disable_allowed_hosts(),
    );
    let bearer = Bearer {
        sources,
        resource_metadata: server.resource_metadata_url(),
    };
    Router::new()
        .route_service(MCP_PATH, service)
        .route_layer(middleware::from_fn_with_state(bearer, bearer_caller))
}

/// What resolves the caller of a request by its access token.
#[derive(Clone)]
struct Bearer {
    sources: Sources,
    /// Where a request that has no caller is pointed to.
    resource_metadata: String,
}

/// The access token `headers` bear, if they bear one.
/// <https://www.rfc-editor.org/rfc/rfc6750#section-2.1>
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let (scheme, token) = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// Puts the [`Caller`] that `request`'s access token acts as now into its
/// extensions, or else answers 401 with the challenge, saying the token is
/// invalid where there is one, and logs why; or 500 where the token does
/// not read.
/// <https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization#authorization-server-location>,
/// <https://www.rfc-editor.org/rfc/rfc6750#section-3>
async fn bearer_caller(State(bearer): State<Bearer>, mut request: Request, next: Next) -> Response {
    let resolved = match bearer_token(request.headers()) {
        Some(token) => AccessTokens::new(bearer.sources.runs.database().clone())
            .resolve(token, bearer.sources.clock.now())
            .await
            .map(|caller| caller.ok_or("its access token is not valid")),
        None => Ok(Err("it has no access token")),
    };
    let reason = match resolved {
        Ok(Ok(caller)) => {
            request.extensions_mut().insert(caller);
            return next.run(request).await;
        }
        Ok(Err(reason)) => reason,
        Err(e) => {
            let error = format!("{e:#}");
            error!(error, "reading an access token failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response();
        }
    };
    info!(reason, "refused a request to the MCP server");
    let mut challenge = format!(r#"Bearer resource_metadata="{}""#, bearer.resource_metadata);
    if bearer_token(request.headers()).is_some() {
        challenge.push_str(r#", error="invalid_token""#);
    }
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            HeaderValue::try_from(challenge).expect("a URL is a header value"),
        )],
    )
        .into_response()
}

/// The server each request is answered by.
#[derive(Clone)]
struct Handler {
    sources: Sources,
    manager: Option<ManagerName>,
}

/// The caller [`bearer_caller`] resolved for the request of `context`.
fn caller(context: &RequestContext<RoleServer>) -> Result<Caller, ErrorData> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Caller>())
        .cloned()
        .ok_or_else(|| ErrorData::internal_error("the request has no caller", None))
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("linewright", env!("CARGO_PKG_VERSION")))
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&PROTOCOL_VERSIONS)
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        tools::named(name)
    }

    fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> {
        std::future::ready(
            caller(&context).map(|caller| ListToolsResult::with_all_items(tools::allowed(&caller))),
        )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let caller = caller(&context)?;
        let acting = tools::Context {
            sources: &self.sources,
            manager: self.manager.as_ref(),
        };
        let called = tools::call(&acting, &caller, request).await?;
        Ok(match called {
            Ok(value) => CallToolResult::structured(value),
            Err(said) => CallToolResult::error(vec![ContentBlock::text(said)]),
        }
        .into())
    }
}
