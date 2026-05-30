use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-roundto";
pub const CONCERN: &str = "rounding: round to N decimal places";
pub const DEPENDS_ON: &[&str] = &["srvcs-isnumber"];

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub isnumber_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    #[schema(value_type = Object)]
    pub value: Value,
    #[schema(value_type = Object)]
    pub decimals: Value,
}

#[derive(Serialize, ToSchema)]
pub struct RoundToResponse {
    #[schema(value_type = Object)]
    pub value: Value,
    #[schema(value_type = Object)]
    pub decimals: Value,
    pub result: f64,
}

/// The single concern: round `v` to `d` decimal places.
///
/// `roundto(3.14159, 2) == 3.14`, `roundto(2.71828, 3) == 2.718`.
pub fn round_to(v: f64, d: i64) -> f64 {
    let factor = 10f64.powi(d as i32);
    (v * factor).round() / factor
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

fn invalid(reason: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": reason })),
    )
        .into_response()
}

/// Forward a dependency's response verbatim (used to propagate `422` for invalid
/// input, so roundto reports the same rejection its dependency did).
fn forward(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, Json(body)).into_response()
}

/// Validate one value-like field by asking `srvcs-isnumber`, mapping its
/// failures to the response this service should return.
async fn ask(url: &str, value: &Value, dependency: &str) -> Result<(), Response> {
    match client::call(url, &json!({ "value": value })).await {
        Err(DepError::Unreachable) => Err(degraded(dependency)),
        Ok((200, body)) => {
            let is_number = body.get("result").and_then(Value::as_bool).unwrap_or(false);
            if is_number {
                Ok(())
            } else {
                Err(invalid("value is not a number"))
            }
        }
        // Invalid input propagates from the leaf dependency; forward it.
        Ok((422, body)) => Err(forward(422, body)),
        Ok(_) => Err(degraded(dependency)),
    }
}

/// `POST /` — compute `round_to(value, decimals)`.
///
/// This is a leaf: it delegates "is this a number" for `value` to
/// `srvcs-isnumber` over HTTP (the single source of truth), then performs the
/// rounding locally in f64. `decimals` is a small count read locally as an i64
/// (>= 0). If the dependency is unreachable, this service reports itself
/// degraded rather than guessing.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = RoundToResponse),
        (status = 422, description = "value is not a number, or decimals is not a non-negative integer"),
        (status = 500, description = "value passed validation but is not representable as a number"),
        (status = 503, description = "a dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    // 1. Delegate "is this a number" for value to srvcs-isnumber.
    if let Err(resp) = ask(&deps.isnumber_url, &req.value, "srvcs-isnumber").await {
        return resp;
    }

    // 2. decimals is a small count read locally as an i64 (>= 0).
    let Some(d) = req.decimals.as_i64() else {
        return invalid("decimals must be an integer");
    };
    if d < 0 {
        return invalid("decimals must be >= 0");
    }

    // 3. value validated as a number; rounding is local f64 arithmetic.
    let Some(v) = req.value.as_f64() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "value validated as a number but is not representable as f64" })),
        )
            .into_response();
    };

    let result = round_to(v, d);
    (
        StatusCode::OK,
        Json(json!({ "value": req.value, "decimals": req.decimals, "result": result })),
    )
        .into_response()
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, RoundToResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
#[allow(clippy::approx_constant)] // deliberate rounding examples from the spec
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    #[test]
    fn rounds_to_n_decimal_places() {
        assert!((round_to(3.14159, 2) - 3.14).abs() < 1e-9);
        assert!((round_to(2.71828, 3) - 2.718).abs() < 1e-9);
        assert!((round_to(5.0, 0) - 5.0).abs() < 1e-9);
        assert!((round_to(2.5, 0) - 3.0).abs() < 1e-9);
        assert!((round_to(-3.14159, 2) - (-3.14)).abs() < 1e-9);
    }

    #[tokio::test]
    async fn index_reports_dependency() {
        let Json(info) = index().await;
        assert_eq!(info.service, "srvcs-roundto");
        assert_eq!(info.concern, "rounding: round to N decimal places");
        assert_eq!(info.depends_on, vec!["srvcs-isnumber"]);
    }
}
