//! A project of the Ellode platform reached through its web console, at
//! `<console>/api/platform/projects/<project>`, with a personal access
//! token made on the console's Access tokens page. The console forwards
//! the JSON API but not the live socket, so such a remote is polled, and
//! it keeps the project's connections itself: none are ever sent.

use anyhow::{Result, anyhow, bail};

/// Where a console forwards a project's API, before the project's name.
const CONSOLE_API: &str = "/api/platform/projects/";

/// How an access token begins.
const TOKEN_PREFIX: &str = "ellode_pat_";

/// A console remote: where the console is and which project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformRemote {
    /// The console's host, with its port when it names one.
    pub host: String,
    pub project: String,
}

/// Whether `name` is a platform project's: lower-case letters, digits and
/// `-`, at most 40, not starting with `-` — the console's own rule.
pub fn is_project_name(name: &str) -> bool {
    name.len() <= 40
        && name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The console remote `url` names, if it is one.
pub fn platform_remote(url: &str) -> Option<PlatformRemote> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.query().is_some() {
        return None;
    }
    let (_, project) = parsed
        .path()
        .trim_end_matches('/')
        .rsplit_once(CONSOLE_API)?;
    if !is_project_name(project) {
        return None;
    }
    let host = parsed.host_str()?;
    Some(PlatformRemote {
        host: match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        },
        project: project.to_owned(),
    })
}

/// The console's address and, when the text names one, a project — from
/// what a person pastes: the console's address, a page of one of its
/// projects (`…/p/shop/runs`), or the remote URL its token dialog shows.
/// Without a scheme, https.
pub fn console_address(text: &str) -> Result<(String, Option<String>)> {
    let text = text.trim();
    if text.is_empty() {
        bail!("give the console's address, as you open it in a browser");
    }
    let with_scheme = if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let parsed = url::Url::parse(&with_scheme)
        .map_err(|error| anyhow!("the console's address does not read: {error}"))?;
    if !matches!(parsed.scheme(), "https" | "http") || parsed.host_str().is_none() {
        bail!("the console's address is an https URL");
    }
    let path = parsed.path().trim_end_matches('/');
    let (prefix, project) = if let Some((prefix, project)) = path.rsplit_once(CONSOLE_API) {
        (prefix, Some(project))
    } else if let Some((prefix, rest)) = path.split_once("/p/") {
        (prefix, rest.split('/').next())
    } else {
        (path, None)
    };
    let project = project
        .filter(|project| is_project_name(project))
        .map(str::to_owned);
    let origin = parsed.origin().ascii_serialization();
    Ok((format!("{origin}{prefix}"), project))
}

/// The remote URL of `project` on the console at `console` (an address
/// from [`console_address`]).
pub fn remote_url(console: &str, project: &str) -> String {
    format!("{}{CONSOLE_API}{project}", console.trim_end_matches('/'))
}

/// The variable a console remote's token is kept in by default.
pub fn token_var_for(project: &str) -> String {
    format!(
        "ELLODE_PAT_{}",
        project
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    )
}

/// The access token `text` holds, trimmed, when it is one as the console
/// shows it: `ellode_pat_` and 43 characters of base64url.
pub fn access_token(text: &str) -> Option<&str> {
    let token = text.trim();
    let body = token.strip_prefix(TOKEN_PREFIX)?;
    (body.len() == 43
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(token)
}

/// What the console lists a token by: `ellode_pat_` and 6 characters, not
/// a secret.
pub fn token_prefix(token: &str) -> &str {
    token.get(..TOKEN_PREFIX.len() + 6).unwrap_or(token)
}

/// `.env` with `var` set to `value`: its first line for `var` (the one
/// that counts) rewritten, `export` kept, or a line added; every other line
/// left as it is.
pub fn with_env_line(existing: &str, var: &str, value: &str) -> String {
    let mut found = false;
    let mut lines: Vec<String> = existing
        .lines()
        .map(|line| {
            let indent = &line[..line.len() - line.trim_start().len()];
            let bare = line.trim_start();
            let (export, assignment) = match bare.strip_prefix("export ") {
                Some(rest) => ("export ", rest.trim_start()),
                None => ("", bare),
            };
            let names_var = assignment
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == var);
            if names_var && !found {
                found = true;
                format!("{indent}{export}{var}={value}")
            } else {
                line.to_owned()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{var}={value}"));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The 401 a console answers for a token it does not take, as the IDE says
/// it: what is wrong and where a new one is made.
pub fn token_refused(remote: &PlatformRemote) -> String {
    format!(
        "{} refused the access token: it is invalid, expired or revoked. Make a new one for \
         project {} on the console's Access tokens page, and paste it in this server's settings.",
        remote.host, remote.project
    )
}

/// Whether `error` is a console refusing the token itself (401).
pub fn is_token_refused(error: &anyhow::Error) -> bool {
    el_engine::server::RemoteRefusal::of(error).is_some_and(|refusal| refusal.status == 401)
}

/// A console's refusal said for the IDE: a 401 is the token, a 403 the role
/// it acts with. None for any other failure, which reads well as it is.
pub fn refusal_text(remote: &PlatformRemote, error: &anyhow::Error) -> Option<String> {
    let refusal = el_engine::server::RemoteRefusal::of(error)?;
    match refusal.status {
        401 => Some(token_refused(remote)),
        403 if refusal.message.contains("role") => Some(format!(
            "{} refused: {}. A token acts with at most the role it was made with, and never \
             more than yours in project {}.",
            remote.host, refusal.message, remote.project
        )),
        403 => Some(format!("{} refused: {}.", remote.host, refusal.message)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_console_remote_is_known_by_its_path() {
        assert_eq!(
            platform_remote("https://el.example.com/api/platform/projects/shop"),
            Some(PlatformRemote {
                host: "el.example.com".into(),
                project: "shop".into()
            })
        );
        assert_eq!(
            platform_remote("http://localhost:3100/api/platform/projects/default/"),
            Some(PlatformRemote {
                host: "localhost:3100".into(),
                project: "default".into()
            })
        );
        // Behind a path prefix, still a console.
        assert_eq!(
            platform_remote("https://tools.example.com/ellode/api/platform/projects/a-1")
                .map(|remote| remote.project),
            Some("a-1".into())
        );
        // The platform server itself, a daemon, a route below a project.
        for url in [
            "http://127.0.0.1:7431/projects/shop",
            "https://el.example.com:7431",
            "https://el.example.com/api/platform/projects/shop/runs",
            "https://el.example.com/api/platform/projects/Shop",
            "https://el.example.com/api/platform/projects/shop?x=1",
        ] {
            assert_eq!(platform_remote(url), None, "{url}");
        }
    }

    #[test]
    fn the_console_address_is_read_from_what_people_paste() {
        let read = |text: &str| console_address(text).unwrap();
        assert_eq!(
            read("el.example.com"),
            ("https://el.example.com".into(), None)
        );
        assert_eq!(
            read("https://el.example.com/"),
            ("https://el.example.com".into(), None)
        );
        assert_eq!(
            read("https://el.example.com/p/shop/pipelines?view=code"),
            ("https://el.example.com".into(), Some("shop".into()))
        );
        assert_eq!(
            read(" https://el.example.com/api/platform/projects/shop "),
            ("https://el.example.com".into(), Some("shop".into()))
        );
        assert_eq!(
            read("http://localhost:3100"),
            ("http://localhost:3100".into(), None)
        );
        assert!(console_address("").is_err());
        assert!(console_address("ftp://el.example.com").is_err());
        assert_eq!(
            remote_url("https://el.example.com/", "shop"),
            "https://el.example.com/api/platform/projects/shop"
        );
        // What a remote's URL becomes reads back as the same console remote.
        let (console, _) = read("el.example.com:8443");
        assert_eq!(
            platform_remote(&remote_url(&console, "shop")),
            Some(PlatformRemote {
                host: "el.example.com:8443".into(),
                project: "shop".into()
            })
        );
    }

    #[test]
    fn project_names_and_token_variables_follow_the_console() {
        assert!(is_project_name("shop"));
        assert!(is_project_name("crm-2"));
        assert!(!is_project_name("-shop"));
        assert!(!is_project_name("Shop"));
        assert!(!is_project_name("shop_eu"));
        assert!(!is_project_name(&"a".repeat(41)));
        assert_eq!(token_var_for("crm-2"), "ELLODE_PAT_CRM_2");
    }

    #[test]
    fn only_an_access_token_is_taken_from_the_clipboard() {
        let token = format!("ellode_pat_{}", "Ab-_".repeat(10) + "xyz");
        assert_eq!(access_token(&format!("  {token}\n")), Some(token.as_str()));
        assert_eq!(token_prefix(&token), "ellode_pat_Ab-_Ab");
        assert_eq!(access_token("ellode_pat_short"), None);
        assert_eq!(access_token(&format!("Bearer {token}")), None);
        assert_eq!(access_token(&token.replace('A', "+")), None);
        assert_eq!(access_token("some copied sentence"), None);
    }

    #[test]
    fn a_console_refusal_names_the_token_or_the_role() {
        let shop = PlatformRemote {
            host: "el.example.com".into(),
            project: "shop".into(),
        };
        let refused = |status: u16, message: &str| {
            anyhow::Error::from(el_engine::server::RemoteRefusal {
                status,
                message: message.into(),
                code: None,
            })
            .context("deploying")
        };
        let expired = refused(401, "the access token is invalid, expired or revoked");
        assert!(is_token_refused(&expired));
        assert_eq!(refusal_text(&shop, &expired), Some(token_refused(&shop)));
        let role = refusal_text(&shop, &refused(403, "your role does not allow this")).unwrap();
        assert!(
            role.contains("never more than yours in project shop"),
            "{role}"
        );
        assert_eq!(
            refusal_text(&shop, &refused(403, "this access token is for project crm")),
            Some("el.example.com refused: this access token is for project crm.".into())
        );
        let other = refused(409, "orders is running");
        assert!(!is_token_refused(&other));
        assert_eq!(refusal_text(&shop, &other), None);
        assert_eq!(
            refusal_text(&shop, &anyhow::anyhow!("remote unreachable")),
            None
        );
    }

    #[test]
    fn a_token_line_is_rewritten_in_place_or_added() {
        assert_eq!(
            with_env_line("", "ELLODE_PAT_SHOP", "new"),
            "ELLODE_PAT_SHOP=new\n"
        );
        assert_eq!(
            with_env_line("A=1\nB=2", "ELLODE_PAT_SHOP", "new"),
            "A=1\nB=2\nELLODE_PAT_SHOP=new\n"
        );
        // The first definition is the one that counts: it is the one changed.
        assert_eq!(
            with_env_line(
                "# tokens\nexport ELLODE_PAT_SHOP=old\nX=1\nELLODE_PAT_SHOP=older\n",
                "ELLODE_PAT_SHOP",
                "new"
            ),
            "# tokens\nexport ELLODE_PAT_SHOP=new\nX=1\nELLODE_PAT_SHOP=older\n"
        );
        // A variable whose name only starts the same is another variable.
        assert_eq!(
            with_env_line("ELLODE_PAT_SHOP_EU=x\n", "ELLODE_PAT_SHOP", "new"),
            "ELLODE_PAT_SHOP_EU=x\nELLODE_PAT_SHOP=new\n"
        );
    }
}
