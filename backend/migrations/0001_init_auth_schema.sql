-- Generic RBAC auth schema with users, roles, permissions, OAuth, email verification, password reset.
-- Schema `auth` is created by docker/postgres-init/01-init-auth-schema.sh (first volume init).
-- IF NOT EXISTS kept here as defense for environments that did not run the init script.

CREATE SCHEMA IF NOT EXISTS auth;

CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA auth;
CREATE EXTENSION IF NOT EXISTS citext   WITH SCHEMA auth;

-- Roles ----------------------------------------------------------------
CREATE TABLE auth.roles (
    id          SMALLSERIAL PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT
);

INSERT INTO auth.roles (name, description) VALUES
    ('admin',     'Full access; can manage users and permissions'),
    ('moderator', 'Can manage shared resources'),
    ('user',      'Regular user');

-- Permissions ----------------------------------------------------------
CREATE TABLE auth.permissions (
    id          SERIAL PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT
);

INSERT INTO auth.permissions (name, description) VALUES
    ('users.read',          'View user list'),
    ('users.manage',        'Create/edit/disable users'),
    ('users.assign_roles',  'Assign roles to users');

CREATE TABLE auth.role_permissions (
    role_id       SMALLINT NOT NULL REFERENCES auth.roles(id)       ON DELETE CASCADE,
    permission_id INTEGER  NOT NULL REFERENCES auth.permissions(id) ON DELETE CASCADE,
    PRIMARY KEY (role_id, permission_id)
);

-- Default role -> permissions mapping
INSERT INTO auth.role_permissions (role_id, permission_id)
SELECT r.id, p.id FROM auth.roles r CROSS JOIN auth.permissions p
WHERE r.name = 'admin';

INSERT INTO auth.role_permissions (role_id, permission_id)
SELECT r.id, p.id FROM auth.roles r, auth.permissions p
WHERE r.name = 'moderator'
  AND p.name IN ('users.read');

-- Users ----------------------------------------------------------------
CREATE TABLE auth.users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email           CITEXT UNIQUE,
    password_hash   TEXT,                    -- NULL for OAuth-only users
    display_name    TEXT,
    role_id         SMALLINT NOT NULL REFERENCES auth.roles(id),
    is_active       BOOLEAN NOT NULL DEFAULT TRUE,
    email_verified  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX users_role_idx ON auth.users(role_id);

-- OAuth identities (one user can link multiple providers) --------------
CREATE TABLE auth.oauth_identities (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    provider    TEXT NOT NULL,        -- 'google', ...
    subject     TEXT NOT NULL,        -- provider-specific user id
    email       TEXT,
    raw_profile JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, subject)
);

CREATE INDEX oauth_identities_user_idx ON auth.oauth_identities(user_id);

-- Refresh tokens (rotated; revocation by setting revoked_at) -----------
CREATE TABLE auth.refresh_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE, -- store hash, not raw token
    issued_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL,
    revoked_at  TIMESTAMPTZ,
    user_agent  TEXT,
    ip          INET
);

CREATE INDEX refresh_tokens_user_idx ON auth.refresh_tokens(user_id);
CREATE INDEX refresh_tokens_expires_idx ON auth.refresh_tokens(expires_at);

-- Email verification tokens
CREATE TABLE auth.email_verification_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE,
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX email_verification_tokens_user_idx ON auth.email_verification_tokens(user_id);

-- Password reset tokens
CREATE TABLE auth.password_reset_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE,
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX password_reset_tokens_user_idx ON auth.password_reset_tokens(user_id);

-- Per-user permission overrides (optional, in addition to role) --------
CREATE TABLE auth.user_permissions (
    user_id       UUID    NOT NULL REFERENCES auth.users(id)       ON DELETE CASCADE,
    permission_id INTEGER NOT NULL REFERENCES auth.permissions(id) ON DELETE CASCADE,
    granted       BOOLEAN NOT NULL DEFAULT TRUE, -- false = explicit revoke
    PRIMARY KEY (user_id, permission_id)
);
