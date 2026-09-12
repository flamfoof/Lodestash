//! Where an endpoint actually is, and what that does (and does not) permit (v1 plan S10).
//!
//! Two facts were conflated before: *locality* (does this request leave the machine?) and *price*
//! (does this endpoint bill?). They were decided together by asking whether the URL string
//! contained `localhost` — which `https://localhost.example.com/v1` satisfies while being someone
//! else's server on the internet, and which `http://[::1]:1234` does not satisfy in every spelling.
//!
//! So the host is parsed out of the URL and judged as a host. Price stays a separate question with
//! its own answer (AI-PIPELINE §5.1): a free remote endpoint is still remote, and a self-hosted
//! server that costs nothing is still the one place bytes may go without asking.

/// Where an endpoint's bytes end up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locality {
    /// This machine: the request never reaches a network the user does not own.
    Loopback,
    /// Anywhere else — including a LAN address, which is someone else's machine even when it is
    /// in the same room.
    Remote,
}

/// The host of `url`, lower-cased, with any userinfo, port and brackets removed.
///
/// `None` when there is no authority to speak of — a relative path, or a scheme with nothing after
/// it. A caller that cannot name the host must treat the destination as remote.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://").map(|(_, r)| r).unwrap_or("");
    if rest.is_empty() {
        return None;
    }
    // The authority ends at the first `/`, `?` or `#`; a path may contain anything, including a
    // word that looks like a host (`https://evil.example/localhost`).
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    // Userinfo runs to the *last* `@`: `http://localhost@evil.example/` is a request to
    // evil.example with `localhost` as a username.
    let hostport = match authority.rsplit_once('@') {
        Some((_, after)) => after,
        None => authority.as_str(),
    };
    if hostport.is_empty() {
        return None;
    }
    // An IPv6 literal is bracketed, and only then may it contain colons.
    if let Some(rest) = hostport.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host.to_string());
    }
    Some(
        hostport
            .split_once(':')
            .map_or(hostport, |(host, _)| host)
            .to_string(),
    )
}

/// Whether `url` resolves to this machine by its own text — loopback literals and the one name
/// reserved for them (RFC 6761 §6.3: `localhost` and anything under `.localhost`).
///
/// Deliberately textual: a DNS name that *resolves* to 127.0.0.1 today can resolve elsewhere on the
/// next request, and this decision guards whether bytes may be sent at all.
pub fn locality(url: &str) -> Locality {
    let Some(host) = host_of(url) else {
        return Locality::Remote;
    };
    let loopback = host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || host == "0:0:0:0:0:0:0:1"
        || host
            .strip_prefix("127.")
            .is_some_and(|rest| rest.split('.').count() == 3);
    if loopback {
        Locality::Loopback
    } else {
        Locality::Remote
    }
}

/// Does this endpoint leave the machine?
pub fn is_remote(url: &str) -> bool {
    locality(url) == Locality::Remote
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases the substring check got wrong, and the spellings it missed.
    #[test]
    fn locality_is_decided_by_the_host_not_by_the_spelling() {
        for local in [
            "http://localhost:1234/v1",
            "http://LOCALHOST:1234/v1",
            "http://127.0.0.1:11434",
            "http://127.1.2.3:8000/v1",
            "http://[::1]:1234/v1",
            "http://api.localhost:8080/v1",
            // Userinfo runs to the *last* `@`, as browsers and curl read it.
            "http://user@name@localhost:1234/v1",
        ] {
            assert_eq!(locality(local), Locality::Loopback, "{local}");
        }
        for remote in [
            // Someone else's server, named to look like yours.
            "https://localhost.example.com/v1",
            "https://my-localhost.ai/v1",
            // Only the reserved name and its subdomains — a suffix without the dot is
            // simply a different name someone registered.
            "https://mylocalhost/v1",
            // The word in a path or a query, not a host.
            "https://evil.example/localhost",
            // A path may contain an `@`; that does not make what follows the host.
            "https://evil.example/redirect@localhost",
            "https://evil.example/v1?host=127.0.0.1",
            // Userinfo runs to the last `@`.
            "http://localhost@evil.example/v1",
            "http://127.0.0.1@evil.example/v1",
            // A LAN address is another machine.
            "http://192.168.1.10:1234/v1",
            "https://api.openai.com/v1",
            // Nothing nameable is not a licence.
            "localhost:1234",
            "",
        ] {
            assert_eq!(locality(remote), Locality::Remote, "{remote}");
        }
    }

    #[test]
    fn the_host_is_extracted_without_port_userinfo_or_brackets() {
        assert_eq!(
            host_of("https://api.openai.com/v1").as_deref(),
            Some("api.openai.com")
        );
        assert_eq!(
            host_of("http://user:pw@example.com:8080/x").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            host_of("http://[2001:db8::1]:443/v1").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(host_of("http://").as_deref(), None);
        assert_eq!(host_of("not a url").as_deref(), None);
    }
}
