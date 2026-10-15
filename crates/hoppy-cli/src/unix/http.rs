//! HTTPS GET for the update check, through `curl` or `wget` (one of them is on every
//! desktop distribution, and macOS ships `/usr/bin/curl`; no TLS stack in Hoppy itself).
//! https only: curl may follow
//! redirects to https URLs only, wget follows none (`--max-redirect=0`).
//! The URL is a separate argv entry, never shell text.

use std::io::Read;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::which;
use crate::platform::{HttpSink, split_https_url};

pub fn curl_args(url: &str, user_agent: &str) -> Vec<String> {
    [
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "20",
        "--max-time",
        "600",
        "--user-agent",
        user_agent,
        url,
    ]
    .map(String::from)
    .to_vec()
}

pub fn wget_args(url: &str, user_agent: &str) -> Vec<String> {
    vec![
        "--quiet".into(),
        "--output-document=-".into(),
        "--https-only".into(),
        "--max-redirect=0".into(),
        "--timeout=20".into(),
        "--tries=2".into(),
        format!("--user-agent={user_agent}"),
        url.into(),
    ]
}

pub fn http_get(url: &str, user_agent: &str, sink: &mut HttpSink<'_>) -> Result<()> {
    if split_https_url(url).is_none() {
        bail!("not a plain https URL");
    }
    let (program, args) = if which("curl").is_some() {
        ("curl", curl_args(url, user_agent))
    } else if which("wget").is_some() {
        ("wget", wget_args(url, user_agent))
    } else {
        bail!("neither curl nor wget is installed");
    };
    let mut child = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {program}"))?;
    let mut out = child.stdout.take().context("no output pipe")?;
    let mut buf = vec![0u8; 64 * 1024];
    let streamed = loop {
        match out.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                if let Err(e) = sink(&buf[..n], None) {
                    break Err(e);
                }
            }
            Err(e) => break Err(e.into()),
        }
    };
    if streamed.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    streamed?;
    if !status.success() {
        bail!("{program} failed ({status})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_only_with_the_url_last() {
        let c = curl_args("https://api.github.com/x", "Hoppy/1.0");
        assert_eq!(
            c.last().map(String::as_str),
            Some("https://api.github.com/x")
        );
        assert!(c.windows(2).any(|w| w == ["--proto-redir", "=https"]));
        let w = wget_args("https://github.com/y", "Hoppy/1.0");
        assert!(w.contains(&"--https-only".to_string()));
        assert!(w.contains(&"--max-redirect=0".to_string()));
        assert_eq!(w.last().map(String::as_str), Some("https://github.com/y"));
    }

    #[test]
    fn refuses_other_urls() {
        let mut sink = |_: &[u8], _: Option<u64>| Ok(());
        assert!(http_get("http://example.com/", "UA", &mut sink).is_err());
        assert!(http_get("https://user@host/", "UA", &mut sink).is_err());
        assert!(http_get("file:///etc/passwd", "UA", &mut sink).is_err());
    }
}
