# srvcs-roundto

The round-to-N-decimal-places primitive of the srvcs.cloud distributed standard
library.

Its single concern: **round a number to `N` decimal places**. It does not
validate input itself — it delegates "is this a number" to
[`srvcs-isnumber`](https://github.com/srvcs/isnumber) over HTTP, the single
source of truth for that question, then performs the rounding locally in `f64`.

Floats are valid input and the result may be fractional. The result is computed
as `(value * 10^decimals).round() / 10^decimals`, so `roundto(3.14159, 2) ==
3.14` and `roundto(2.71828, 3) == 2.718`.

If `srvcs-isnumber` is unreachable, `srvcs-roundto` reports itself **degraded
(503)** rather than guessing.

## API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/` | Service identity, concern, and dependency list |
| `POST` | `/` | Compute `roundto(value, decimals)` |
| `GET` | `/healthz` `/readyz` `/metrics` `/openapi.json` | srvcs service standard surface |

```sh
curl -s -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"value": 3.14159, "decimals": 2}'
# {"value":3.14159,"decimals":2,"result":3.14}
```

Responses:

- `200 {"value": n, "decimals": d, "result": <float>}` — evaluated.
- `422` — the value is not a number (per `srvcs-isnumber`), or `decimals` is not
  a non-negative integer.
- `503` — a dependency is unavailable.

## Dependencies

- [`srvcs-isnumber`](https://github.com/srvcs/isnumber) — input validation.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SRVCS_BIND_ADDR` | `0.0.0.0:8080` | Bind address |
| `SRVCS_ISNUMBER_URL` | `http://127.0.0.1:8081` | Base URL of `srvcs-isnumber` |
| `SRVCS_ENV` | `development` | Environment label for logs |
| `RUST_LOG` | `info,tower_http=info` | Tracing filter |

## Local checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Orchestration tests stand up a mock `srvcs-isnumber` in-process (one that
genuinely computes "is this a number" from the request body), so the suite runs
without the rest of the fleet. Float results are compared approximately
(`|got - expected| < 1e-9`). See
[`srvcs/platform`](https://github.com/srvcs/platform) for the shared standard.

> Note: the `cargoHash` in `flake.nix` is inherited from the template and must be
> refreshed with a `nix build` before the Nix gates pass.
