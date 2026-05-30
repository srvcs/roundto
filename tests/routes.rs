// Tests use spec rounding examples (pi/e-like literals) on purpose.
#![allow(clippy::approx_constant)]

use axum::body::Body;
use axum::extract::Json as ExtractJson;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use srvcs_roundto::{api::Deps, health, router, telemetry};
use tower::ServiceExt;

/// Spin up a mock dependency that answers `POST /` with a fixed status + body,
/// and return its base URL. Lets us test orchestration without the real fleet.
async fn spawn_mock(status: StatusCode, body: Value) -> String {
    let app = AxumRouter::new().route(
        "/",
        post(move || {
            let body = body.clone();
            async move { (status, Json(body)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// A *computing* mock of srvcs-isnumber: it reads the request body and actually
/// decides whether `value` is a JSON number, returning the real verdict. This
/// genuinely exercises roundto's validation branch (number -> proceed,
/// non-number -> 422) rather than rubber-stamping a fixed answer.
async fn spawn_isnumber() -> String {
    async fn handler(ExtractJson(req): ExtractJson<Value>) -> Json<Value> {
        let value = req.get("value").cloned().unwrap_or(Value::Null);
        let is_number = value.is_number();
        Json(json!({ "value": value, "result": is_number }))
    }
    let app = AxumRouter::new().route("/", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn app(isnumber_url: &str) -> axum::Router {
    router(
        telemetry::metrics_handle_for_tests(),
        Deps {
            isnumber_url: isnumber_url.to_string(),
        },
    )
}

async fn eval(isnumber_url: &str, value: Value, decimals: Value) -> (StatusCode, Value) {
    let res = app(isnumber_url)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "value": value, "decimals": decimals }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Approximate float comparison per the service contract.
fn approx(got: &Value, expected: f64) {
    let got = got.as_f64().expect("result is a float");
    assert!(
        (got - expected).abs() < 1e-9,
        "expected ~{expected}, got {got}"
    );
}

// A base URL with nothing listening — exercises the degraded path.
const DEAD_URL: &str = "http://127.0.0.1:1";

async fn status_of(uri: &str) -> StatusCode {
    app(DEAD_URL)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn index_ok() {
    assert_eq!(status_of("/").await, StatusCode::OK);
}

#[tokio::test]
async fn healthz_ok() {
    assert_eq!(status_of("/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn readyz_reflects_state() {
    health::set_ready(true);
    assert_eq!(status_of("/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn openapi_ok() {
    assert_eq!(status_of("/openapi.json").await, StatusCode::OK);
}

#[tokio::test]
async fn rounds_pi_to_two_places() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(3.14159), json!(2)).await;
    assert_eq!(status, StatusCode::OK);
    approx(&body["result"], 3.14);
    assert_eq!(body["value"], 3.14159);
    assert_eq!(body["decimals"], 2);
}

#[tokio::test]
async fn rounds_e_to_three_places() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(2.71828), json!(3)).await;
    assert_eq!(status, StatusCode::OK);
    approx(&body["result"], 2.718);
}

#[tokio::test]
async fn rounds_integer_input() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(5), json!(2)).await;
    assert_eq!(status, StatusCode::OK);
    approx(&body["result"], 5.0);
}

#[tokio::test]
async fn rounds_to_zero_places() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(2.5), json!(0)).await;
    assert_eq!(status, StatusCode::OK);
    approx(&body["result"], 3.0);
}

#[tokio::test]
async fn rejects_non_number() {
    // The computing mock genuinely reports "nope" is not a number -> 422.
    let isnumber = spawn_isnumber().await;
    let (status, _) = eval(&isnumber, json!("nope"), json!(2)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rejects_when_isnumber_says_false() {
    // Fixed-answer mock asserting the false branch maps to 422.
    let isnumber = spawn_mock(StatusCode::OK, json!({ "result": false })).await;
    let (status, _) = eval(&isnumber, json!(3.14159), json!(2)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rejects_negative_decimals() {
    let isnumber = spawn_isnumber().await;
    let (status, _) = eval(&isnumber, json!(3.14159), json!(-1)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn degrades_when_isnumber_is_unreachable() {
    let (status, body) = eval(DEAD_URL, json!(3.14159), json!(2)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-isnumber");
}
