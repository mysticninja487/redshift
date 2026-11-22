//! Update check: asks GitHub for the newest published release and compares it with this version.
//!
//! What is sent: an ordinary HTTPS request to `api.github.com` carrying the app's name and
//! version. No account, no identifiers, nothing about the library. Redshift only *tells* you
//! about a new version; it only downloads and installs one when the user presses the button for it
//! (see `Net::download` and `App::download_update`).

use std::cmp::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Net, Request};
use crate::error::{Error, Result};

pub const HOST: &str = "api.github.com";
/// Where a release file is served from: GitHub, which redirects to its file storage.
pub const DOWNLOAD_HOSTS: [&str; 3] = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
pub const REPO: &str = "mysticninja487/redshift";
const BASE: &str = "https://api.github.com";
const TTL: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateInfo {
    /// The version that is running.
    pub current: String,
    /// The newest published release, without a leading "v"; `None` when the project has none yet.
    pub latest: Option<String>,
    /// True when `latest` is newer than `current`.
    pub newer: bool,
    /// The release page to open.
    pub url: String,
    /// The release notes, shortened.
    pub notes: String,
    /// What "Update now" would download for how this copy is installed; `None` when the release has
    /// no suitable file (then only the release page is offered). Set by the app, not by `check`.
    pub asset: Option<UpdateAsset>,
    /// The installer and the bare program attached to the release, if there.
    pub installer: Option<UpdateAsset>,
    pub portable: Option<UpdateAsset>,
}

/// A file attached to a release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// SHA-256 as lowercase hex, when GitHub states it.
    pub sha256: Option<String>,
    /// "installer" or "portable".
    pub kind: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Release {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

fn download_prefix() -> String {
    format!("https://github.com/{REPO}/releases/download/")
}

/// Whether an address is a file attached to this project's own releases (nothing else is downloaded).
pub fn is_release_download(url: &str) -> bool {
    url.starts_with(&download_prefix()) && !url.contains("..") && !url.contains(char::is_whitespace)
}

fn asset_of(a: &ReleaseAsset, kind: &str) -> Option<UpdateAsset> {
    // A real file of a sensible size, from this project's own release.
    if !is_release_download(&a.browser_download_url) || a.size == 0 || a.size > MAX_UPDATE_BYTES {
        return None;
    }
    let sha256 = a.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")).filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).map(str::to_ascii_lowercase);
    Some(UpdateAsset { name: a.name.clone(), url: a.browser_download_url.clone(), size: a.size, sha256, kind: kind.into() })
}

/// The largest update file that is accepted.
pub const MAX_UPDATE_BYTES: u64 = 300 * 1024 * 1024;

/// Splits "v0.5.10-beta.1" into ([0, 5, 10], has_suffix).
fn parse_version(v: &str) -> (Vec<u64>, bool) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let (core, pre) = match v.split_once(['-', '+']) {
        Some((c, _)) => (c, true),
        None => (v, false),
    };
    (core.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect(), pre)
}

/// Orders two version strings numerically per part (so 0.5.10 is newer than 0.5.9). A version with
/// a suffix such as `-beta` comes before the same version without one.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let ((va, pa), (vb, pb)) = (parse_version(a), parse_version(b));
    let n = va.len().max(vb.len());
    for i in 0..n {
        let (x, y) = (va.get(i).copied().unwrap_or(0), vb.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    pb.cmp(&pa)
}

pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases")
}

pub fn check(net: &Net, current: &str, force: bool) -> Result<UpdateInfo> {
    check_at(net, BASE, current, force)
}

pub(crate) fn check_at(net: &Net, base: &str, current: &str, force: bool) -> Result<UpdateInfo> {
    let url = format!("{base}/repos/{REPO}/releases/latest");
    let req = Request::new(&url, "Update check").cached_for(if force { Duration::ZERO } else { TTL });
    let r = net.get(&req)?;
    let none = |url: String| UpdateInfo { current: current.to_string(), latest: None, newer: false, url, notes: String::new(), asset: None, installer: None, portable: None };
    match r.status {
        404 => Ok(none(releases_page())),
        200 => {
            let rel: Release = serde_json::from_str(&r.body).map_err(|e| Error::Network(format!("unreadable release reply: {e}")))?;
            if rel.tag_name.is_empty() || rel.draft || rel.prerelease {
                return Ok(none(releases_page()));
            }
            let latest = rel.tag_name.trim().trim_start_matches(['v', 'V']).to_string();
            let url = if rel.html_url.starts_with("https://github.com/") { rel.html_url } else { releases_page() };
            let notes: String = rel.body.unwrap_or_default().chars().take(1500).collect();
            let lower = |a: &ReleaseAsset| a.name.to_ascii_lowercase();
            let installer = rel.assets.iter().find(|a| lower(a).ends_with("setup.exe")).and_then(|a| asset_of(a, "installer"));
            let portable = rel.assets.iter().find(|a| lower(a).contains("portable") && lower(a).ends_with(".exe")).and_then(|a| asset_of(a, "portable"));
            Ok(UpdateInfo { current: current.to_string(), newer: compare_versions(&latest, current) == Ordering::Greater, latest: Some(latest), url, notes, asset: None, installer, portable })
        }
        403 | 429 => Err(Error::Network("GitHub is limiting requests right now; try again later".into())),
        s => Err(Error::Network(format!("GitHub answered {s}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testserver::*;
    use super::*;

    fn net(dir: &std::path::Path) -> Net {
        let n = Net::new(dir.join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        n
    }

    #[test]
    fn versions_compare_by_number_not_text() {
        use Ordering::*;
        assert_eq!(compare_versions("0.5.10", "0.5.9"), Greater);
        assert_eq!(compare_versions("v0.6.0", "0.5.10"), Greater);
        assert_eq!(compare_versions("0.6", "0.6.0"), Equal);
        assert_eq!(compare_versions("1.0.0", "0.99.99"), Greater);
        assert_eq!(compare_versions("0.6.0-beta.1", "0.6.0"), Less);
        assert_eq!(compare_versions("0.6.0", "0.6.0-beta.1"), Greater);
        assert_eq!(compare_versions("0.5.9", "0.5.9"), Equal);
    }

    #[test]
    fn newer_release_is_reported_with_its_page() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, r#"{"tag_name":"v0.6.1","html_url":"https://github.com/mysticninja487/redshift/releases/tag/v0.6.1","body":"Fixes","draft":false,"prerelease":false}"#)]);
        let n = net(d.path());
        let u = check_at(&n, &s.base, "0.6.0", false).unwrap();
        assert_eq!((u.latest.as_deref(), u.newer, u.notes.as_str()), (Some("0.6.1"), true, "Fixes"));
        assert!(u.url.ends_with("/v0.6.1"));
        assert!(s.requests.lock()[0].starts_with("GET /repos/mysticninja487/redshift/releases/latest "));
    }

    #[test]
    fn release_files_are_picked_and_foreign_ones_ignored() {
        let d = tempfile::tempdir().unwrap();
        let hash = "a".repeat(64);
        let body = format!(r#"{{"tag_name":"v0.6.5","html_url":"https://github.com/mysticninja487/redshift/releases/tag/v0.6.5","assets":[
            {{"name":"Redshift_0.6.5_x64-setup.exe","browser_download_url":"https://github.com/mysticninja487/redshift/releases/download/v0.6.5/Redshift_0.6.5_x64-setup.exe","size":9000000,"digest":"sha256:{hash}"}},
            {{"name":"Redshift-portable.exe","browser_download_url":"https://github.com/mysticninja487/redshift/releases/download/v0.6.5/Redshift-portable.exe","size":20000000}},
            {{"name":"evil-setup.exe","browser_download_url":"https://evil.example.com/evil-setup.exe","size":5}}]}}"#);
        let s = start(vec![reply(200, &body)]);
        let u = check_at(&net(d.path()), &s.base, "0.6.4", false).unwrap();
        let i = u.installer.unwrap();
        assert_eq!((i.name.as_str(), i.size, i.sha256.as_deref(), i.kind.as_str()), ("Redshift_0.6.5_x64-setup.exe", 9_000_000, Some(hash.as_str()), "installer"));
        let p = u.portable.unwrap();
        assert_eq!((p.name.as_str(), p.sha256), ("Redshift-portable.exe", None));
        assert!(is_release_download(&i.url));
        assert!(!is_release_download("https://github.com/someone/else/releases/download/v1/x.exe"));
        assert!(!is_release_download("https://github.com/mysticninja487/redshift/releases/download/../../x.exe"));
    }

    #[test]
    fn same_or_older_release_is_not_an_update() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, r#"{"tag_name":"v0.5.10","html_url":"https://github.com/x/y/releases/tag/v0.5.10"}"#)]);
        let n = net(d.path());
        assert!(!check_at(&n, &s.base, "0.6.0", false).unwrap().newer);
        assert!(!check_at(&n, &s.base, "0.5.10", true).unwrap().newer);
    }

    #[test]
    fn no_releases_yet_is_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(404, r#"{"message":"Not Found"}"#)]);
        let n = net(d.path());
        let u = check_at(&n, &s.base, "0.6.0", false).unwrap();
        assert_eq!((u.latest, u.newer), (None, false));
        assert_eq!(u.url, releases_page());
    }

    #[test]
    fn drafts_prereleases_and_foreign_links_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let n = net(d.path());
        let s = start(vec![reply(200, r#"{"tag_name":"v9.9.9","html_url":"https://github.com/a/b","prerelease":true}"#)]);
        assert!(!check_at(&n, &s.base, "0.6.0", true).unwrap().newer);
        let s = start(vec![reply(200, r#"{"tag_name":"v9.9.9","html_url":"https://evil.example/download.exe"}"#)]);
        let u = check_at(&n, &s.base, "0.6.0", true).unwrap();
        assert!(u.newer);
        assert_eq!(u.url, releases_page(), "only GitHub links are passed on");
    }

    #[test]
    fn rate_limit_and_garbage_are_errors() {
        let d = tempfile::tempdir().unwrap();
        let n = net(d.path());
        let s = start(vec![reply(403, "rate limited")]);
        assert!(check_at(&n, &s.base, "0.6.0", true).is_err());
        let s = start(vec![reply(200, "<html>")]);
        assert!(check_at(&n, &s.base, "0.6.0", true).is_err());
    }
}
