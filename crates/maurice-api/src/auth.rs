//! Who is calling and what they may do (ADR-011).
//!
//! A principal is resolved per request from a session cookie or a bearer
//! token. Handlers state what they need (`Need`) and `authorize` decides:
//! scope (read / plan / apply), role (viewer / operator / admin), the
//! environment policy (apply in prod and staging needs an admin behind a
//! human token) and token workspace restrictions.

use crate::crypto;
use crate::proto::maurice::v1::*;
use crate::services::State;
use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult,
};
use maurice_core::Env as CoreEnv;
use maurice_db::{AuditRow, SessionRow, TokenRow, UserRow};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "maurice_session";
pub const SESSION_IDLE: Duration = Duration::hours(12);
/// Header a CLI sends to name its safety tier (ADR-007); recorded in the audit log.
pub const TIER_HEADER: &str = "maurice-tier";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer,
    Operator,
    Admin,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "operator" => Some(Role::Operator),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Operator => "operator",
            Role::Viewer => "viewer",
        }
    }
    pub fn to_proto(self) -> crate::proto::maurice::v1::Role {
        use crate::proto::maurice::v1::Role as P;
        match self {
            Role::Admin => P::ROLE_ADMIN,
            Role::Operator => P::ROLE_OPERATOR,
            Role::Viewer => P::ROLE_VIEWER,
        }
    }
    pub fn from_proto(r: buffa::EnumValue<crate::proto::maurice::v1::Role>) -> Option<Role> {
        use crate::proto::maurice::v1::Role as P;
        match r.as_known() {
            Some(P::ROLE_ADMIN) => Some(Role::Admin),
            Some(P::ROLE_OPERATOR) => Some(Role::Operator),
            Some(P::ROLE_VIEWER) => Some(Role::Viewer),
            _ => None,
        }
    }
    /// Widest scope a role may ever use.
    fn max_scope(self) -> Scope {
        match self {
            Role::Viewer => Scope::Read,
            Role::Operator | Role::Admin => Scope::Apply,
        }
    }
}

/// Cumulative: apply implies plan implies read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Read,
    Plan,
    Apply,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "read" => Some(Scope::Read),
            "plan" => Some(Scope::Plan),
            "apply" => Some(Scope::Apply),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Plan => "plan",
            Scope::Apply => "apply",
        }
    }
    pub fn to_proto(self) -> crate::proto::maurice::v1::Scope {
        use crate::proto::maurice::v1::Scope as P;
        match self {
            Scope::Read => P::SCOPE_READ,
            Scope::Plan => P::SCOPE_PLAN,
            Scope::Apply => P::SCOPE_APPLY,
        }
    }
    pub fn from_proto(s: buffa::EnumValue<crate::proto::maurice::v1::Scope>) -> Option<Scope> {
        use crate::proto::maurice::v1::Scope as P;
        match s.as_known() {
            Some(P::SCOPE_READ) => Some(Scope::Read),
            Some(P::SCOPE_PLAN) => Some(Scope::Plan),
            Some(P::SCOPE_APPLY) => Some(Scope::Apply),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Via {
    Session { hash: Vec<u8> },
    Token { id: Uuid, name: String },
}

/// The caller of one request.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: Uuid,
    pub login: String,
    pub role: Role,
    /// Widest scope this credential carries.
    pub scope: Scope,
    /// Empty = every workspace.
    pub workspace_ids: Vec<Uuid>,
    pub human: bool,
    pub via: Via,
    pub tier: String,
}

impl Principal {
    /// `login` for sessions, `login/token:name` for tokens.
    pub fn name(&self) -> String {
        match &self.via {
            Via::Session { .. } => self.login.clone(),
            Via::Token { name, .. } => format!("{}/token:{name}", self.login),
        }
    }
    pub fn is_session(&self) -> bool {
        matches!(self.via, Via::Session { .. })
    }
    pub fn can_see(&self, workspace: Uuid) -> bool {
        self.workspace_ids.is_empty() || self.workspace_ids.contains(&workspace)
    }
}

/// What a handler needs from the caller.
#[derive(Debug, Clone, Copy, Default)]
pub struct Need {
    pub scope: Option<Scope>,
    pub env: Option<CoreEnv>,
    pub workspace: Option<Uuid>,
    pub admin: bool,
}

impl Need {
    pub fn read() -> Self {
        Need {
            scope: Some(Scope::Read),
            ..Default::default()
        }
    }
    pub fn plan() -> Self {
        Need {
            scope: Some(Scope::Plan),
            ..Default::default()
        }
    }
    pub fn apply() -> Self {
        Need {
            scope: Some(Scope::Apply),
            ..Default::default()
        }
    }
    pub fn admin(mut self) -> Self {
        self.admin = true;
        self
    }
    pub fn env(mut self, env: CoreEnv) -> Self {
        self.env = Some(env);
        self
    }
    pub fn env_opt(mut self, env: Option<CoreEnv>) -> Self {
        self.env = env;
        self
    }
    pub fn workspace(mut self, ws: Uuid) -> Self {
        self.workspace = Some(ws);
        self
    }
}

fn denied(msg: impl Into<String>) -> ConnectError {
    ConnectError::new(ErrorCode::PermissionDenied, msg)
}

fn unauthenticated(msg: &str) -> ConnectError {
    ConnectError::new(ErrorCode::Unauthenticated, msg)
}

fn internal(e: impl std::fmt::Display) -> ConnectError {
    tracing::error!(error = %e, "internal");
    ConnectError::new(ErrorCode::Internal, e.to_string())
}

/// The policy. Pure, so it is testable without a database.
pub fn authorize(p: &Principal, need: &Need) -> Result<(), ConnectError> {
    if need.admin && p.role != Role::Admin {
        return Err(denied("this action is for administrators"));
    }
    if let Some(ws) = need.workspace
        && !p.can_see(ws)
    {
        return Err(denied("this token is not allowed on that workspace"));
    }
    let Some(scope) = need.scope else {
        return Ok(());
    };
    if p.scope < scope {
        return Err(denied(format!(
            "this credential has no {} scope",
            scope.as_str()
        )));
    }
    if p.role.max_scope() < scope {
        return Err(denied(format!(
            "role {} cannot {}",
            p.role.as_str(),
            scope.as_str()
        )));
    }
    if scope == Scope::Apply
        && let Some(env) = need.env
        && env.human_only()
    {
        if !p.human {
            return Err(denied(format!(
                "apply in {env} needs a human behind the token"
            )));
        }
        if p.role != Role::Admin {
            return Err(denied(format!(
                "role {} may plan in {env} but not apply; ask an administrator",
                p.role.as_str()
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

/// Random password for a first admin or a reset: 20 url-safe characters.
pub fn generate_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut raw = [0u8; 20];
    crypto::random_bytes(&mut raw).expect("urandom");
    raw.iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

pub fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// A fresh token secret: `mrk_` plus 40 hex characters.
pub fn new_token_secret() -> String {
    format!("mrk_{}", crypto::random_hex(20))
}

fn new_session_secret() -> String {
    crypto::random_hex(32)
}

fn cookie_value(ctx: &RequestContext) -> Option<String> {
    let raw = ctx.header("cookie")?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == SESSION_COOKIE).then(|| v.trim().to_string())
    })
}

fn bearer(ctx: &RequestContext) -> Option<String> {
    let raw = ctx.header("authorization")?.to_str().ok()?;
    raw.strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .map(|t| t.trim().to_string())
}

fn request_is_https(ctx: &RequestContext) -> bool {
    ctx.header("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

fn set_cookie(secret: &str, secure: bool, max_age: i64) -> String {
    format!(
        "{SESSION_COOKIE}={secret}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

fn tier_of(ctx: &RequestContext) -> String {
    ctx.header(TIER_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn principal_of_user(u: &UserRow, via: Via, tier: String) -> Result<Principal, ConnectError> {
    if u.disabled {
        return Err(unauthenticated("account is disabled"));
    }
    let role = Role::parse(&u.role).ok_or_else(|| internal("unknown role"))?;
    Ok(Principal {
        user_id: u.id,
        login: u.login.clone(),
        role,
        scope: role.max_scope(),
        workspace_ids: vec![],
        human: true,
        via,
        tier,
    })
}

/// Resolve the caller from the request: bearer token first, then the session cookie.
pub async fn resolve(state: &State, ctx: &RequestContext) -> Result<Principal, ConnectError> {
    let tier = tier_of(ctx);
    if let Some(secret) = bearer(ctx) {
        let Some(t) = state
            .store
            .touch_token(&sha256(secret.as_bytes()))
            .await
            .map_err(internal)?
        else {
            return Err(unauthenticated("token is unknown, revoked or expired"));
        };
        let Some(u) = state.store.get_user(t.user_id).await.map_err(internal)? else {
            return Err(unauthenticated("token owner no longer exists"));
        };
        let mut p = principal_of_user(
            &u,
            Via::Token {
                id: t.id,
                name: t.name.clone(),
            },
            tier,
        )?;
        p.scope = t
            .scopes
            .iter()
            .filter_map(|s| Scope::parse(s))
            .max()
            .unwrap_or(Scope::Read)
            .min(p.role.max_scope());
        p.workspace_ids = t.workspace_ids.clone();
        p.human = t.human;
        return Ok(p);
    }
    if let Some(secret) = cookie_value(ctx) {
        let hash = sha256(secret.as_bytes());
        let Some(s) = state
            .store
            .touch_session(&hash, SESSION_IDLE)
            .await
            .map_err(internal)?
        else {
            return Err(unauthenticated("session expired; sign in again"));
        };
        let Some(u) = state.store.get_user(s.user_id).await.map_err(internal)? else {
            return Err(unauthenticated("session owner no longer exists"));
        };
        return principal_of_user(&u, Via::Session { hash }, tier);
    }
    Err(unauthenticated("sign in first"))
}

/// Write one audit row. Failures are logged, never surfaced: the action already happened.
pub async fn audit(
    state: &State,
    p: Option<&Principal>,
    action: &str,
    target: impl Into<String>,
    outcome: &str,
    detail: impl Into<String>,
) {
    let row = AuditRow {
        id: Uuid::now_v7(),
        at: OffsetDateTime::now_utc(),
        user_id: p.map(|p| p.user_id),
        principal: p.map(|p| p.name()).unwrap_or_else(|| "anonymous".into()),
        action: action.into(),
        target: target.into(),
        tier: p.map(|p| p.tier.clone()).unwrap_or_default(),
        outcome: outcome.into(),
        detail: detail.into(),
    };
    if let Err(e) = state.store.insert_audit(&row).await {
        tracing::error!(error = %e, action, "audit row not written");
    }
}

// ---------------------------------------------------------------------------
// Auth service
// ---------------------------------------------------------------------------

pub struct AuthSvc {
    pub state: Arc<State>,
}

fn ts(t: OffsetDateTime) -> buffa_types::Timestamp {
    buffa_types::Timestamp {
        seconds: t.unix_timestamp(),
        nanos: 0,
        ..Default::default()
    }
}

fn user_to_proto(u: &UserRow) -> User {
    User {
        id: u.id.to_string(),
        login: u.login.clone(),
        role: Role::parse(&u.role)
            .map(Role::to_proto)
            .unwrap_or(crate::proto::maurice::v1::Role::ROLE_UNSPECIFIED)
            .into(),
        disabled: u.disabled,
        created_at: ts(u.created_at).into(),
        ..Default::default()
    }
}

fn token_to_proto(t: &TokenRow, owner: &str) -> TokenInfo {
    TokenInfo {
        id: t.id.to_string(),
        name: t.name.clone(),
        owner: owner.into(),
        scopes: t
            .scopes
            .iter()
            .filter_map(|s| Scope::parse(s))
            .map(|s| s.to_proto().into())
            .collect(),
        workspace_ids: t.workspace_ids.iter().map(|w| w.to_string()).collect(),
        human: t.human,
        created_at: ts(t.created_at).into(),
        expires_at: match t.expires_at {
            Some(t) => ts(t).into(),
            None => buffa::MessageField::none(),
        },
        last_used_at: match t.last_used_at {
            Some(t) => ts(t).into(),
            None => buffa::MessageField::none(),
        },
        ..Default::default()
    }
}

fn invalid(msg: impl Into<String>) -> ConnectError {
    ConnectError::new(ErrorCode::InvalidArgument, msg)
}

fn scopes_for(role: Role) -> Vec<String> {
    let mut out = vec!["read".to_string()];
    if role.max_scope() >= Scope::Plan {
        out.push("plan".into());
    }
    if role.max_scope() >= Scope::Apply {
        out.push("apply".into());
    }
    out
}

impl AuthSvc {
    async fn mint_token(
        &self,
        user: &UserRow,
        name: &str,
        scopes: Vec<String>,
        workspace_ids: Vec<Uuid>,
        human: bool,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<(TokenRow, String), ConnectError> {
        let secret = new_token_secret();
        let row = TokenRow {
            id: Uuid::now_v7(),
            user_id: user.id,
            name: name.into(),
            hash: sha256(secret.as_bytes()),
            scopes,
            workspace_ids,
            human,
            created_at: OffsetDateTime::now_utc(),
            expires_at,
            last_used_at: None,
            revoked_at: None,
        };
        self.state
            .store
            .insert_token(&row)
            .await
            .map_err(internal)?;
        Ok((row, secret))
    }
}

#[allow(refining_impl_trait)]
impl Auth for AuthSvc {
    async fn login(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, LoginRequest>,
    ) -> ServiceResult<LoginResponse> {
        let login = req.login.trim().to_ascii_lowercase();
        let user = self
            .state
            .store
            .get_user_by_login(&login)
            .await
            .map_err(internal)?;
        // Verify against a dummy hash when the user is unknown so timing does not leak logins.
        let ok = match &user {
            Some(u) => verify_password(req.password, &u.password_hash),
            None => {
                let _ = verify_password(req.password, DUMMY_HASH);
                false
            }
        };
        let Some(user) = user.filter(|_| ok) else {
            audit(
                &self.state,
                None,
                "login",
                &login,
                "denied",
                "bad credentials",
            )
            .await;
            return Err(unauthenticated("wrong login or password"));
        };
        if user.disabled {
            audit(
                &self.state,
                None,
                "login",
                &login,
                "denied",
                "account disabled",
            )
            .await;
            return Err(unauthenticated("account is disabled"));
        }
        let role = Role::parse(&user.role).ok_or_else(|| internal("unknown role"))?;
        let secret = new_session_secret();
        let now = OffsetDateTime::now_utc();
        self.state
            .store
            .insert_session(&SessionRow {
                id: Uuid::now_v7(),
                user_id: user.id,
                hash: sha256(secret.as_bytes()),
                created_at: now,
                expires_at: now + SESSION_IDLE,
                last_seen_at: now,
            })
            .await
            .map_err(internal)?;
        let mut token = String::new();
        if req.create_token {
            let name = if req.token_name.is_empty() {
                "cli"
            } else {
                req.token_name
            };
            let (_, secret) = self
                .mint_token(&user, name, scopes_for(role), vec![], true, None)
                .await?;
            token = secret;
        }
        let p = principal_of_user(&user, Via::Session { hash: vec![] }, tier_of(&ctx))?;
        audit(
            &self.state,
            Some(&p),
            "login",
            &login,
            "ok",
            if req.create_token {
                "with cli token"
            } else {
                ""
            },
        )
        .await;
        Ok(Response::new(LoginResponse {
            user: user_to_proto(&user).into(),
            token,
            ..Default::default()
        })
        .with_header(
            "set-cookie",
            set_cookie(
                &secret,
                request_is_https(&ctx),
                SESSION_IDLE.whole_seconds(),
            ),
        ))
    }

    async fn logout(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, LogoutRequest>,
    ) -> ServiceResult<Empty> {
        if let Some(secret) = cookie_value(&ctx) {
            self.state
                .store
                .delete_session(&sha256(secret.as_bytes()))
                .await
                .map_err(internal)?;
        }
        Ok(Response::new(Empty::default())
            .with_header("set-cookie", set_cookie("", request_is_https(&ctx), 0)))
    }

    async fn me(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, MeRequest>,
    ) -> ServiceResult<User> {
        let p = resolve(&self.state, &ctx).await?;
        let user = self
            .state
            .store
            .get_user(p.user_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| unauthenticated("user vanished"))?;
        Response::ok(user_to_proto(&user))
    }

    async fn create_token(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, CreateTokenRequest>,
    ) -> ServiceResult<CreateTokenResponse> {
        let p = resolve(&self.state, &ctx).await?;
        if !p.is_session() {
            return Err(denied("tokens are created from a browser session"));
        }
        let name = req.name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(invalid("token name must be 1-64 characters"));
        }
        let mut scopes: Vec<Scope> = req
            .scopes
            .iter()
            .filter_map(|s| Scope::from_proto(*s))
            .collect();
        scopes.sort();
        scopes.dedup();
        if scopes.is_empty() {
            return Err(invalid("pick at least one scope"));
        }
        if scopes.iter().any(|s| *s > p.role.max_scope()) {
            return Err(denied(format!(
                "role {} cannot grant that scope",
                p.role.as_str()
            )));
        }
        let mut workspace_ids = Vec::new();
        for w in &req.workspace_ids {
            workspace_ids
                .push(Uuid::parse_str(w).map_err(|_| invalid("workspace ids must be uuids"))?);
        }
        let expires_at = (req.expires_days > 0)
            .then(|| OffsetDateTime::now_utc() + Duration::days(req.expires_days as i64));
        let user = self
            .state
            .store
            .get_user(p.user_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| unauthenticated("user vanished"))?;
        let (row, secret) = self
            .mint_token(
                &user,
                name,
                scopes.iter().map(|s| s.as_str().to_string()).collect(),
                workspace_ids,
                req.human,
                expires_at,
            )
            .await?;
        audit(&self.state, Some(&p), "token.create", name, "ok", "").await;
        Response::ok(CreateTokenResponse {
            token: token_to_proto(&row, &user.login).into(),
            secret,
            ..Default::default()
        })
    }

    async fn list_tokens(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, ListTokensRequest>,
    ) -> ServiceResult<ListTokensResponse> {
        let p = resolve(&self.state, &ctx).await?;
        let owner_filter = (p.role != Role::Admin).then_some(p.user_id);
        let rows = self
            .state
            .store
            .list_tokens(owner_filter)
            .await
            .map_err(internal)?;
        let users = self.state.store.list_users().await.map_err(internal)?;
        let login_of = |id: Uuid| {
            users
                .iter()
                .find(|u| u.id == id)
                .map(|u| u.login.clone())
                .unwrap_or_default()
        };
        Response::ok(ListTokensResponse {
            tokens: rows
                .iter()
                .map(|t| token_to_proto(t, &login_of(t.user_id)))
                .collect(),
            ..Default::default()
        })
    }

    async fn revoke_token(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, RevokeTokenRequest>,
    ) -> ServiceResult<Empty> {
        let p = resolve(&self.state, &ctx).await?;
        let id = Uuid::parse_str(req.id).map_err(|_| invalid("token id must be a uuid"))?;
        let Some(t) = self.state.store.get_token(id).await.map_err(internal)? else {
            return Err(ConnectError::new(ErrorCode::NotFound, "token not found"));
        };
        if t.user_id != p.user_id && p.role != Role::Admin {
            return Err(denied("not your token"));
        }
        self.state.store.revoke_token(id).await.map_err(internal)?;
        audit(&self.state, Some(&p), "token.revoke", &t.name, "ok", "").await;
        Response::ok(Empty::default())
    }

    async fn list_users(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, ListUsersRequest>,
    ) -> ServiceResult<ListUsersResponse> {
        let p = resolve(&self.state, &ctx).await?;
        authorize(&p, &Need::read().admin())?;
        let rows = self.state.store.list_users().await.map_err(internal)?;
        Response::ok(ListUsersResponse {
            users: rows.iter().map(user_to_proto).collect(),
            ..Default::default()
        })
    }

    async fn create_user(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, CreateUserRequest>,
    ) -> ServiceResult<User> {
        let p = resolve(&self.state, &ctx).await?;
        authorize(&p, &Need::apply().admin())?;
        if !p.is_session() {
            return Err(denied("users are managed from a browser session"));
        }
        let login = req.login.trim().to_ascii_lowercase();
        if !valid_login(&login) {
            return Err(invalid(
                "login must be lowercase letters, digits, dots, dashes or underscores, 1-63 chars",
            ));
        }
        if req.password.len() < 12 {
            return Err(invalid("password must be at least 12 characters"));
        }
        let role = Role::from_proto(req.role).ok_or_else(|| invalid("role is required"))?;
        if self
            .state
            .store
            .get_user_by_login(&login)
            .await
            .map_err(internal)?
            .is_some()
        {
            return Err(ConnectError::new(
                ErrorCode::AlreadyExists,
                "login already taken",
            ));
        }
        let row = UserRow {
            id: Uuid::now_v7(),
            login: login.clone(),
            password_hash: hash_password(req.password).map_err(internal)?,
            role: role.as_str().into(),
            disabled: false,
            created_at: OffsetDateTime::now_utc(),
        };
        self.state.store.insert_user(&row).await.map_err(internal)?;
        audit(
            &self.state,
            Some(&p),
            "user.create",
            &login,
            "ok",
            role.as_str(),
        )
        .await;
        Response::ok(user_to_proto(&row))
    }

    async fn update_user(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, UpdateUserRequest>,
    ) -> ServiceResult<User> {
        let p = resolve(&self.state, &ctx).await?;
        authorize(&p, &Need::apply().admin())?;
        if !p.is_session() {
            return Err(denied("users are managed from a browser session"));
        }
        let id = Uuid::parse_str(req.id).map_err(|_| invalid("user id must be a uuid"))?;
        let Some(u) = self.state.store.get_user(id).await.map_err(internal)? else {
            return Err(ConnectError::new(ErrorCode::NotFound, "user not found"));
        };
        let role = Role::from_proto(req.role)
            .or_else(|| Role::parse(&u.role))
            .ok_or_else(|| internal("unknown role"))?;
        // Never lock everyone out: the last enabled admin stays an enabled admin.
        let was_admin = u.role == "admin" && !u.disabled;
        let stays_admin = role == Role::Admin && !req.disabled;
        if was_admin
            && !stays_admin
            && self
                .state
                .store
                .count_enabled_admins()
                .await
                .map_err(internal)?
                <= 1
        {
            return Err(ConnectError::new(
                ErrorCode::FailedPrecondition,
                "this is the last administrator",
            ));
        }
        let password_hash = if req.password.is_empty() {
            None
        } else {
            if req.password.len() < 12 {
                return Err(invalid("password must be at least 12 characters"));
            }
            Some(hash_password(req.password).map_err(internal)?)
        };
        self.state
            .store
            .update_user(id, role.as_str(), req.disabled, password_hash.as_deref())
            .await
            .map_err(internal)?;
        if req.disabled || password_hash.is_some() {
            self.state
                .store
                .delete_sessions_of(id)
                .await
                .map_err(internal)?;
        }
        audit(
            &self.state,
            Some(&p),
            "user.update",
            &u.login,
            "ok",
            format!(
                "role {}{}{}",
                role.as_str(),
                if req.disabled { ", disabled" } else { "" },
                if password_hash.is_some() {
                    ", password reset"
                } else {
                    ""
                }
            ),
        )
        .await;
        let updated = self
            .state
            .store
            .get_user(id)
            .await
            .map_err(internal)?
            .ok_or_else(|| internal("user vanished"))?;
        Response::ok(user_to_proto(&updated))
    }

    async fn list_audit(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, ListAuditRequest>,
    ) -> ServiceResult<ListAuditResponse> {
        let p = resolve(&self.state, &ctx).await?;
        if p.role < Role::Operator {
            return Err(denied("the audit log is for operators and administrators"));
        }
        let limit = if req.limit == 0 {
            100
        } else {
            req.limit.min(1000)
        } as i64;
        let rows = self.state.store.list_audit(limit).await.map_err(internal)?;
        Response::ok(ListAuditResponse {
            entries: rows
                .iter()
                .map(|a| AuditEntry {
                    id: a.id.to_string(),
                    at: ts(a.at).into(),
                    principal: a.principal.clone(),
                    action: a.action.clone(),
                    target: a.target.clone(),
                    tier: a.tier.clone(),
                    outcome: a.outcome.clone(),
                    detail: a.detail.clone(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }
}

/// A valid argon2id hash of nothing useful, verified when a login is unknown so
/// the response time does not reveal which logins exist.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$2zFqk2KH0rbnPeVwTeoO1j9F7qFIBkcQ7lj9tP8hJeM";

pub fn valid_login(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-'
        })
        && s.as_bytes()[0].is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(role: Role, scope: Scope, human: bool, ws: Vec<Uuid>) -> Principal {
        Principal {
            user_id: Uuid::nil(),
            login: "t".into(),
            role,
            scope,
            workspace_ids: ws,
            human,
            via: Via::Session { hash: vec![] },
            tier: String::new(),
        }
    }

    #[test]
    fn viewer_reads_only() {
        let p = principal(Role::Viewer, Scope::Apply, true, vec![]);
        assert!(authorize(&p, &Need::read()).is_ok());
        assert!(authorize(&p, &Need::plan()).is_err());
        assert!(authorize(&p, &Need::apply()).is_err());
    }

    #[test]
    fn operator_applies_in_dev_only() {
        let p = principal(Role::Operator, Scope::Apply, true, vec![]);
        assert!(authorize(&p, &Need::apply().env(CoreEnv::Dev)).is_ok());
        assert!(authorize(&p, &Need::plan().env(CoreEnv::Prod)).is_ok());
        assert!(authorize(&p, &Need::apply().env(CoreEnv::Prod)).is_err());
        assert!(authorize(&p, &Need::apply().env(CoreEnv::Staging)).is_err());
        assert!(authorize(&p, &Need::read().admin()).is_err());
    }

    #[test]
    fn admin_needs_a_human_token_for_prod() {
        let bot = principal(Role::Admin, Scope::Apply, false, vec![]);
        assert!(authorize(&bot, &Need::apply().env(CoreEnv::Dev)).is_ok());
        assert!(authorize(&bot, &Need::apply().env(CoreEnv::Prod)).is_err());
        let human = principal(Role::Admin, Scope::Apply, true, vec![]);
        assert!(authorize(&human, &Need::apply().env(CoreEnv::Prod)).is_ok());
        assert!(authorize(&human, &Need::apply().admin()).is_ok());
    }

    #[test]
    fn token_scope_and_workspaces_limit_the_role() {
        let ws = Uuid::now_v7();
        let other = Uuid::now_v7();
        let p = principal(Role::Admin, Scope::Plan, true, vec![ws]);
        assert!(authorize(&p, &Need::plan().workspace(ws)).is_ok());
        assert!(authorize(&p, &Need::apply().workspace(ws)).is_err());
        assert!(authorize(&p, &Need::read().workspace(other)).is_err());
    }

    #[test]
    fn passwords_round_trip() {
        let h = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &h));
        assert!(!verify_password("wrong", &h));
        assert!(!verify_password("x", "not a hash"));
        assert_eq!(generate_password().len(), 20);
    }

    #[test]
    fn logins() {
        assert!(valid_login("admin"));
        assert!(valid_login("max.k_1-2"));
        assert!(!valid_login("Admin"));
        assert!(!valid_login(".dot"));
        assert!(!valid_login(""));
    }
}
