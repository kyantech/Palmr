# Operator configuration

Palmr reads its infrastructure configuration from `PALMR_*` environment variables once, at startup (ARCHITECTURE §7.2, §8.1 step 1). Product policy — SMTP, branding, quotas, session and password policy, retention — is Admin configuration stored in the database and is never read from the environment.

Palmr boots with no environment variables at all. An empty value (`PALMR_BASE_URL=`) is treated as unset. Enumerated and boolean values are case-insensitive.

## Variables

| Variable | Default | Accepted values |
|---|---|---|
| `PALMR_HOST` | `0.0.0.0` | an IPv4 or IPv6 address |
| `PALMR_PORT` | `5487` | `1`–`65535` |
| `PALMR_BASE_URL` | `http://localhost:<PALMR_PORT>` | absolute `http`/`https` URL with a host; a path is allowed; no query, fragment or credentials |
| `PALMR_DATA_DIR` | `/data` | a directory path |
| `PALMR_TRUST_PROXY` | `off` | `off`, or a comma-separated list of IP addresses/CIDR ranges |
| `PALMR_LOG_LEVEL` | `info` | a log filter directive |
| `PALMR_LOG_FORMAT` | `json` | `json`, `pretty` |
| `PALMR_STORAGE_PROVIDER` | `local` | `local`, `s3` |
| `PALMR_STORAGE_ORPHAN_REAP` | `false` | `true`, `false` |
| `PALMR_DB_READ_CONNECTIONS` | `4` | `1`–`16` |
| `PALMR_DB_SYNCHRONOUS` | `full` | `full`, `normal` |
| `PALMR_ZIP_MAX_ENTRIES` | `100000` | `1`–`10000000` |
| `PALMR_JOB_WORKERS` | `2` | `1`–`64` |
| `PALMR_SHUTDOWN_GRACE_SECS` | `30` | `1`–`3600` |
| `PALMR_UPLOAD_BUFFER_BYTES` | `262144` | `4096`–`16777216` |
| `PALMR_MAX_CONCURRENT_TRANSFERS` | `3` | `1`–`64`; advisory, surfaced to the web app |
| `PALMR_DEFAULT_LANGUAGE` | — | suggested locale for first setup only; never overrides a saved instance default |

### S3 storage (`PALMR_STORAGE_PROVIDER=s3`)

| Variable | Default | Accepted values |
|---|---|---|
| `PALMR_S3_ENDPOINT` | **required** | `http`/`https` URL, host and port only |
| `PALMR_S3_REGION` | **required** | region name |
| `PALMR_S3_BUCKET` | **required** | an existing bucket |
| `PALMR_S3_ACCESS_KEY` | **required** | secret; never logged or displayed |
| `PALMR_S3_SECRET_KEY` | **required** | secret; never logged or displayed |
| `PALMR_S3_PUBLIC_ENDPOINT` | `PALMR_S3_ENDPOINT` | browser-facing `http`/`https` URL, host and port only |
| `PALMR_S3_PROFILE` | `generic` | `generic`, `aws`, `minio`, `r2`, `rustfs`, `b2`, `gcs`, `wasabi`, `garage` |
| `PALMR_S3_FORCE_PATH_STYLE` | `true` | `true`, `false` |
| `PALMR_S3_CA_FILE` | — | path to a PEM bundle trusted by the S3 client only |
| `PALMR_S3_REJECT_UNAUTHORIZED` | `true` | `true`, `false` |
| `PALMR_S3_MULTIPART_TTL_HOURS` | `24` | `1`–`720` |

The provider profile is never inferred from the endpoint hostname: set `PALMR_S3_PROFILE` explicitly.

## Startup failures and warnings

Any invalid value stops startup with `STARTUP_CONFIG_INVALID`. The message lists every offending variable with the value received and the rule it broke. Secret values, and URL values that contain credentials, are reported as `(value withheld)`.

These conditions start normally but are reported as warnings:

| Condition | Warning |
|---|---|
| `PALMR_BASE_URL` unset | `STARTUP_BASE_URL_DEFAULTED`: generated links, e-mail links and OAuth callbacks point at `http://localhost:<PALMR_PORT>`, and browsers reaching Palmr through any other origin have state-changing requests refused. Set `PALMR_BASE_URL` for every reverse-proxy, LAN or public deployment. |
| `PALMR_S3_REJECT_UNAUTHORIZED=false` | TLS verification is disabled for the S3 client only. Prefer `PALMR_S3_CA_FILE`. |
| `PALMR_S3_*` set while the provider is `local` | the S3 variables are ignored |
| an unrecognized `PALMR_*` variable | the variable is ignored; check it for typos |

`PALMR_TRUST_PROXY` has no trust-all mode: `all`, `*`, `0.0.0.0/0` and `::/0` are rejected. `PALMR_UID` and `PALMR_GID` do not exist; run the container with Docker's `user:` instead.

## Background jobs

`PALMR_JOB_WORKERS` sets how many background jobs run at the same time. The number of queued jobs never changes it. Jobs are rows in the `jobs` table of `palmr.db`, so queued work survives restarts and is included in backups.

- A job that fails is retried later: 30 seconds after the first failure, doubling after each further failure up to 6 hours, with ±20 % random spread. A job that runs out of attempts is marked `dead`, logged at `ERROR` as `job.dead_lettered`, and is never retried automatically.
- On `SIGTERM`/`SIGINT` Palmr stops starting new jobs first. Jobs already running get up to `PALMR_SHUTDOWN_GRACE_SECS` to finish. A job still running when the grace period ends stays claimed, and the next start runs it again once its 5-minute lease expires. The same happens after a crash or `SIGKILL`.
- To inspect jobs that failed permanently: `SELECT id, kind, attempts, last_error, updated_at FROM jobs WHERE state = 'dead';`. `last_error` holds an error code and the job id only, never the handler's own error text.
