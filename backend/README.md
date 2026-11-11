# Generic RBAC Auth Service

Standalone, reusable authentication and authorization service written in Rust. Provides user registration, login, JWT tokens, OAuth (Google), email verification, password reset, and role-based access control (RBAC).

**Zero business logic** — this is pure auth infrastructure you can embed in any application.

## Features

- **User management**: Register, login, email verification, password reset, account disable
- **Token management**: JWT access + refresh tokens with rotation and revocation
- **OAuth**: Google OAuth integration with automatic user creation
- **RBAC**: Role-based access control with granular permissions
- **Admin API**: User listing, role assignment, permission grants
- **Email**: Async email notifications (optional; logs to stdout in dev mode)
- **Database**: PostgreSQL with migrations, stateless API design

## Architecture

Cargo workspace:
```
crates/
├── api/         Axum HTTP server + routes
├── auth/        JWT, Argon2, OAuth logic
├── core/        Domain types (User, Role, Permission)
├── db/          SQLx + Postgres repository layer
```

Zero external services needed except PostgreSQL.

## Quick Start

### Prerequisites

- Rust 1.95+ (`rust-toolchain.toml` pins version)
- PostgreSQL 14+ (local or Docker)
- Docker (optional, for postgres container)

### Setup (Local Dev)

Rebuild + restart api:
```
docker compose up -d --build api
```

Build only (no restart):
```
docker compose build api
```

Force clean rebuild (no cache):
```
docker compose build --no-cache api
docker compose up -d api
```


```powershell
# 1. Clone and copy env (edit POSTGRES_*, DATABASE_URL, APP__AUTH__JWT_SECRET as needed)
cp .env.example .env

# 2. Start Postgres (compose reads .env from repo root)
docker compose up -d postgres

# 3. Run migrations (DATABASE_URL points at host port from .env)
$env:DATABASE_URL = (Get-Content .env | Select-String "^DATABASE_URL=").ToString().Split("=",2)[1]
sqlx migrate run

# 4. Start the API
cargo run -p generic-auth-api
```

API listens on `http://localhost:8080`. Health check: `GET /health`.

### Setup (Docker)

Run from repo root (compose auto-reads `.env` adjacent to `docker-compose.yml`):

```powershell
docker compose up -d
```

Postgres: `${POSTGRES_HOST_PORT}` on host → `${POSTGRES_PORT}` in container. API: 8080.

## Production Deployment (Docker)

Docker Compose is the supported deployment path. App auto-runs migrations on container start (compose sets `APP__DB__RUN_MIGRATIONS_ON_START=true`).

### 1. Prerequisites

- Docker Engine 24+ with Compose v2 plugin
- Outbound network access for `cargo` + Docker Hub during initial build
- Committed `.sqlx/` offline query cache (run `cargo sqlx prepare --workspace` locally against a dev DB and commit before first deploy)

### 2. Generate secrets

```bash
# JWT signing secret (64-byte hex)
openssl rand -hex 64

# Postgres password
openssl rand -hex 32
```

Never commit these. Never reuse the defaults in `docker-compose.yml`.

### 3. Create production `.env` at repo root

```env
POSTGRES_USER=auth_user
POSTGRES_PASSWORD=<paste 32-byte hex from step 2>
POSTGRES_DB=auth_db
POSTGRES_PORT=5432
POSTGRES_HOST_PORT=5434

APP__AUTH__JWT_SECRET=<paste 64-byte hex from step 2>
APP__AUTH__JWT_ISSUER=<your-service-name>

# Optional integrations
APP__AUTH__GOOGLE__CLIENT_ID=
APP__AUTH__GOOGLE__CLIENT_SECRET=
APP__AUTH__GOOGLE__REDIRECT_URL=
APP__EMAIL__SMTP_HOST=
APP__EMAIL__SMTP_PORT=587
APP__EMAIL__SMTP_USERNAME=
APP__EMAIL__SMTP_PASSWORD=
```

`.env` is git-ignored. Store securely (Vault, AWS Secrets Manager, etc.).

### 4. Override compose for production

Create `docker-compose.prod.yml` at repo root (hides Postgres from host + passes optional env):

```yaml
services:
  postgres:
    ports: !reset []
  api:
    environment:
      RUST_LOG: info,generic_auth=info,sqlx=warn
      APP__AUTH__GOOGLE__CLIENT_ID: ${APP__AUTH__GOOGLE__CLIENT_ID}
      APP__AUTH__GOOGLE__CLIENT_SECRET: ${APP__AUTH__GOOGLE__CLIENT_SECRET}
      APP__AUTH__GOOGLE__REDIRECT_URL: ${APP__AUTH__GOOGLE__REDIRECT_URL}
      APP__EMAIL__SMTP_HOST: ${APP__EMAIL__SMTP_HOST}
      APP__EMAIL__SMTP_PORT: ${APP__EMAIL__SMTP_PORT}
      APP__EMAIL__SMTP_USERNAME: ${APP__EMAIL__SMTP_USERNAME}
      APP__EMAIL__SMTP_PASSWORD: ${APP__EMAIL__SMTP_PASSWORD}
```

### 5. Build + start (from repo root)

```bash
docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d --build
```

API starts → connects to Postgres → applies migrations → listens on `:8080`.

### 6. Create first admin user

```bash
docker compose exec api /usr/local/bin/generic-auth-api create-admin --email=admin@example.com --password='<strong-password>'
```

### 7. Verify

```bash
curl -fsS http://<host>:8080/health
docker compose ps
docker compose logs -f api
```

### 8. Reverse proxy + TLS

Run nginx / Caddy / Traefik in front of port `8080`. Terminate TLS there. Do not expose `8080` to the public internet directly.

Caddy example:

```caddyfile
auth.example.com {
    reverse_proxy localhost:8080
}
```

### 9. Updates

```bash
git pull
docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d --build
```

Old container stops, new one starts, migrations apply automatically. Downtime: ~2-5s.

### 10. Backups

Postgres data lives in the `generic-auth_postgres_data` named volume. Snapshot via:

```bash
docker compose exec postgres pg_dump -U auth_user auth_db > backup-$(date +%F).sql
```

Restore:

```bash
cat backup-2026-05-15.sql | docker compose exec -T postgres psql -U auth_user auth_db
```

### Production hardening checklist

- [ ] Strong `APP__AUTH__JWT_SECRET` (≥64 bytes random); rotate on suspected compromise
- [ ] Strong `POSTGRES_PASSWORD`; not the default
- [ ] Postgres port not published to host (use `!reset []` override above)
- [ ] TLS via reverse proxy; HSTS enabled
- [ ] `RUST_LOG=info` not `debug` (avoid logging tokens / PII)
- [ ] `cors.allowed_origins` set explicitly in `config/production.toml`
- [ ] SMTP configured (else verification / reset emails only log to stdout)
- [ ] Scheduled `pg_dump` backups + offsite copy
- [ ] Container restart policy `unless-stopped` (already set)
- [ ] Monitor `/health` from external uptime check

## Production Deployment (Docker)

Docker Compose is the supported deployment path. App auto-runs migrations on container start (compose sets `APP__DB__RUN_MIGRATIONS_ON_START=true`).

### 1. Prerequisites

- Docker Engine 24+ with Compose v2 plugin
- Outbound network access for `cargo` + Docker Hub during initial build
- Committed `.sqlx/` offline query cache (run `cargo sqlx prepare --workspace` locally against a dev DB and commit before first deploy)

### 2. Generate secrets

```bash
# JWT signing secret (64-byte hex)
openssl rand -hex 64

# Postgres password
openssl rand -hex 32
```

Never commit these. Never reuse the defaults in `docker/docker-compose.yml`.

### 3. Create production `.env` next to compose file

`docker/.env`:

```env
APP__AUTH__JWT_SECRET=<paste 64-byte hex from step 2>
POSTGRES_PASSWORD=<paste 32-byte hex from step 2>
```

Compose reads it automatically. Add `docker/.env` to local secrets store; it is already excluded via `.dockerignore` / `.gitignore`.

### 4. Override compose for production

Create `docker/docker-compose.prod.yml`:

```yaml
services:
  postgres:
    environment:
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
    ports: !reset []           # do not expose Postgres on host
  api:
    environment:
      DATABASE_URL: postgres://auth_user:${POSTGRES_PASSWORD}@postgres:5432/auth_db
      RUST_LOG: info,generic_auth=info,sqlx=warn
      APP__AUTH__GOOGLE__CLIENT_ID: ${GOOGLE_CLIENT_ID:-}
      APP__AUTH__GOOGLE__CLIENT_SECRET: ${GOOGLE_CLIENT_SECRET:-}
      APP__AUTH__GOOGLE__REDIRECT_URL: ${GOOGLE_REDIRECT_URL:-}
      APP__EMAIL__SMTP_HOST: ${SMTP_HOST:-}
      APP__EMAIL__SMTP_PORT: ${SMTP_PORT:-587}
      APP__EMAIL__SMTP_USERNAME: ${SMTP_USERNAME:-}
      APP__EMAIL__SMTP_PASSWORD: ${SMTP_PASSWORD:-}
```

### 5. Build + start

```bash
cd docker
docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d --build
```

API starts → connects to Postgres → applies migrations → listens on `:8080`.

### 6. Create first admin user

```bash
docker compose exec api /usr/local/bin/generic-auth-api \
  create-admin --email=admin@example.com --password='<strong-password>'
```

### 7. Verify

```bash
curl -fsS http://<host>:8080/health
docker compose ps
docker compose logs -f api
```

### 8. Reverse proxy + TLS

Run nginx / Caddy / Traefik in front of port `8080`. Terminate TLS there. Do not expose `8080` to the public internet directly.

Caddy example:

```caddyfile
auth.example.com {
    reverse_proxy localhost:8080
}
```

### 9. Updates

```bash
git pull
docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d --build
```

Old container stops, new one starts, migrations apply automatically. Downtime: ~2-5s.

### 10. Backups

Postgres data lives in the `generic-auth_postgres_data` named volume. Snapshot via:

```bash
docker compose exec postgres pg_dump -U auth_user auth_db > backup-$(date +%F).sql
```

Restore:

```bash
cat backup-2026-05-15.sql | docker compose exec -T postgres psql -U auth_user auth_db
```

### Production hardening checklist

- [ ] Strong `APP__AUTH__JWT_SECRET` (≥64 bytes random); rotate on suspected compromise
- [ ] Strong `POSTGRES_PASSWORD`; not the default
- [ ] Postgres port not published to host (use `!reset []` override above)
- [ ] TLS via reverse proxy; HSTS enabled
- [ ] `RUST_LOG=info` not `debug` (avoid logging tokens / PII)
- [ ] `cors.allowed_origins` set explicitly in `config/production.toml`
- [ ] SMTP configured (else verification / reset emails only log to stdout)
- [ ] Scheduled `pg_dump` backups + offsite copy
- [ ] Container restart policy `unless-stopped` (already set)
- [ ] Monitor `/health` from external uptime check

## API Endpoints

All auth endpoints return `(access_token, refresh_token)` on success.

### Authentication

| Method | Path | Auth | Body | Description |
|--------|------|------|------|-------------|
| POST | `/api/v1/auth/register` | — | email, password | Register; auto-sends verification email |
| POST | `/api/v1/auth/login` | — | email, password | Login; returns tokens |
| POST | `/api/v1/auth/refresh` | — | refresh_token | Rotate tokens (old token revoked) |
| POST | `/api/v1/auth/logout` | Bearer | — | Revoke current refresh token |
| POST | `/api/v1/auth/verify-email-code` | — | email, code | Verify email with 6-digit code |
| POST | `/api/v1/auth/resend-verification` | — | email | Re-send verification code (rate limited) |
| POST | `/api/v1/auth/forgot-password` | — | email | Send password reset email |
| POST | `/api/v1/auth/reset-password` | — | token, password | Reset password |
| POST | `/api/v1/auth/change-password` | Bearer | current_password, new_password | Change password; revokes all sessions |
| GET | `/api/v1/auth/oauth/google` | — | ?redirect_uri= | Redirect to Google OAuth |
| GET | `/api/v1/auth/oauth/google/callback` | — | ?code= | OAuth callback; exchanges code for tokens |

### Users

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/users/me` | Bearer | Current user profile |
| GET | `/api/v1/users/me/permissions` | Bearer | Current user's effective permissions |

### Admin

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/admin/users` | Bearer | List users (paginated: `?limit=&offset=`); requires `users.read` |
| DELETE | `/api/v1/admin/users/{id}` | Bearer | Permanently delete a user; requires `users.manage` |
| PATCH | `/api/v1/admin/users/{id}/role` | Bearer | Change user role; requires `users.assign_roles` |
| PATCH | `/api/v1/admin/users/{id}/active` | Bearer | Enable/disable account; requires `users.manage` |
| POST | `/api/v1/admin/users/{id}/permissions` | Bearer | Grant/revoke permission; requires `users.assign_roles` |

### Health

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Liveness check |

## Database Schema

Auth-only tables (no business logic):

```sql
-- Roles (admin, moderator, user)
roles (id, name, description)

-- Permissions (users.read, users.manage, users.assign_roles)
permissions (id, name, description)

-- Role → permission mappings
role_permissions (role_id, permission_id)

-- Users
users (id, email, password_hash, display_name, role_id, is_active, email_verified, created_at, updated_at)

-- OAuth identities (one user can have multiple)
oauth_identities (id, user_id, provider, subject, email, raw_profile, created_at)

-- Refresh tokens (rotated, revocable)
refresh_tokens (id, user_id, token_hash, issued_at, expires_at, revoked_at, user_agent, ip)

-- Email verification codes (token_hash = HMAC of a 6-digit code; attempt_count caps guesses)
email_verification_tokens (id, user_id, token_hash, attempt_count, expires_at, used_at, created_at)

-- Password reset tokens
password_reset_tokens (id, user_id, token_hash, expires_at, used_at, created_at)

-- Per-user permission overrides (optional)
user_permissions (user_id, permission_id, granted)
```

## Configuration

Config hierarchy (later overrides earlier):

1. `config/default.toml`
2. `config/<APP_ENV>.toml` (e.g., `production.toml`)
3. Environment variables: `APP__SECTION__KEY` (double underscores = nesting)
4. `DATABASE_URL` (overrides `db.url`)

See `.env.example` for all variables.

### Key Sections

**[auth]**
- `jwt_secret`: Secret for signing JWT tokens. **Required.** Set via `APP__AUTH__JWT_SECRET` env (no toml fallback).
- `jwt_access_ttl_min`: Access token lifetime (default 30 min)
- `jwt_refresh_ttl_days`: Refresh token lifetime (default 14 days)
- `password_min_length`: Minimum password length (default 8)

**[auth.google]** (optional)
- `client_id`, `client_secret`: From Google Cloud Console
- `redirect_url`: Where OAuth callback lands

**[email]** (optional)
- Leave `smtp_host` blank for dev mode → logs links to stdout
- Set for production: `smtp_host`, `smtp_port`, `smtp_username`, `smtp_password`

**[db]**
- `url`: Postgres connection string. **Required.** Set via `DATABASE_URL` env (no toml fallback).
- `max_connections`: Pool size (default 20)
- `min_connections`: Min idle connections (default 2)
- `run_migrations_on_start`: Auto-migrate on startup (default true; production.toml sets false — docker compose overrides via env to true)

## Admin Setup

No admin users are seeded by migrations. Create one via CLI:

```powershell
$env:DATABASE_URL = "postgres://auth_user:auth_password@localhost:5432/auth_db"
cargo run -p generic-auth-api -- create-admin --email=you@example.com --password=yourpassword
```

Or with compiled binary:
```powershell
./generic-auth-api create-admin --email=you@example.com --password=yourpassword
```

Then login at `POST /api/v1/auth/login` with those credentials.

## Testing

```powershell
# Unit tests (no DB needed)
cargo test -p generic-auth-api routes::auth

# All tests
cargo test --workspace
```

Integration tests that touch the DB require:
```powershell
$env:DATABASE_URL = "postgres://auth_user:auth_password@localhost:5432/auth_db_test"
sqlx migrate run --database-url $env:DATABASE_URL
cargo test --workspace
```

## Troubleshooting

**`error: relation "..." does not exist`** when running tests
- Migrations haven't been applied. Run `sqlx migrate run` against the test DB.

**`password authentication failed`** at startup
- Check `DATABASE_URL` env var is set correctly.
- Postgres isn't running or credentials are wrong.

**`SQLX_OFFLINE=true` but no cached data** during Docker build
- For offline compilation, run locally: `cargo sqlx prepare --workspace`
- Commit the `.sqlx/` directory.

## Integration Guide

To embed this in your application:

1. **Add auth as a dependency**:
   ```toml
   generic-auth-auth = { path = "path/to/crates/auth" }
   generic-auth-db = { path = "path/to/crates/db" }
   ```

2. **Initialize auth state** (see `crates/api/src/state.rs`):
   ```rust
   let db = generic_auth_db::connect(&db_config).await?;
   let jwt = JwtCodec::new(jwt_config);
   ```

3. **Protect endpoints** with `#[require_auth]` middleware or JWT validation:
   ```rust
   let user = extract_user_from_token(&request, &jwt)?;
   ```

4. **Check permissions** as needed:
   ```rust
   if !user.has_permission(&db, "resource.write").await? {
       return Err(Unauthorized);
   }
   ```

## API Documentation

Live Swagger UI: `http://localhost:8080/api/docs`

Generated from `#[utoipa::path]` annotations on route handlers.

## License

MIT OR Apache-2.0
