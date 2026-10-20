//! Access control for the record.
//!
//! Audit finding 1: the previous `authed` helper returned `true` whenever the
//! subscribe token was unset. Since `TIDELINE_SUBSCRIBE_TOKEN` is documented as
//! optional and Cloud deployments authenticate through `TIDELINE_AUTH_URL`
//! instead, a default Cloud deployment served every tenant's full record to
//! anyone who asked, with the tenant chosen by the caller's own `?ns=`.
//!
//! Two rules fix it. Reads fall open only when *nothing* is configured, rather
//! than when *one thing* is unset. And the namespace is a function of the
//! credential alone — `?ns=` is gone from the record routes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Granted { ns: String },
    Denied,
}

#[derive(Clone, Debug, Default)]
pub struct AuthMode {
    pub publish_token: Option<String>,
    pub subscribe_token: Option<String>,
    /// A control plane is configured to verify per-tenant keys.
    pub cloud_verify: bool,
}

impl AuthMode {
    /// True only when no credential of any kind is configured — local
    /// development, and nothing else.
    fn wide_open(&self) -> bool {
        self.publish_token.is_none() && self.subscribe_token.is_none() && !self.cloud_verify
    }
}

fn matches(want: &Option<String>, presented: Option<&str>) -> bool {
    match (want, presented) {
        (Some(w), Some(p)) => w == p,
        _ => false,
    }
}

/// Resolve a read.
///
/// `tenant_ns` is the namespace of an already-verified Cloud key. It never comes
/// from a request parameter.
pub fn resolve_read(mode: &AuthMode, presented: Option<&str>, tenant_ns: Option<&str>) -> Access {
    if let Some(ns) = tenant_ns {
        return Access::Granted { ns: ns.to_string() };
    }
    if mode.wide_open()
        || matches(&mode.subscribe_token, presented)
        // A writer may read back what it wrote.
        || matches(&mode.publish_token, presented)
    {
        return Access::Granted { ns: String::new() };
    }
    Access::Denied
}

/// Resolve a write.
pub fn resolve_write(mode: &AuthMode, presented: Option<&str>) -> Access {
    if matches(&mode.publish_token, presented) || mode.wide_open() {
        return Access::Granted { ns: String::new() };
    }
    Access::Denied
}
