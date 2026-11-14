//! Provides the `ConnectInstruction` structure, which allows for a `ParsedDisplay`
//! to be transformed into a server connection.

use super::ParsedDisplay;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A possible address for an X11 server.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectAddress<'a> {
    /// Connect to this hostname and port over TCP.
    Hostname(&'a str, u16),
    /// Connect to this Unix socket.
    ///
    /// First, the given path should be attempted in the abstract namespace. Only if that fails,
    /// then the named socket with the given name should be tried.
    Socket(String),
}

/// Get an iterator over all of the addresses we should target with a
/// `ParsedDisplay`.
pub(super) fn connect_addresses(p: &ParsedDisplay) -> impl Iterator<Item = ConnectAddress<'_>> {
    const TCP_PORT_BASE: u16 = 6000;
    let ParsedDisplay {
        host,
        protocol,
        display,
        ..
    } = p;

    let mut targets = Vec::new();

    if (protocol.is_none() || protocol.as_deref() != Some("unix"))
        && !host.is_empty()
        && host != "unix"
    {
        targets.push(ConnectAddress::Hostname(host, TCP_PORT_BASE + display));
    } else {
        if protocol.is_none() || protocol.as_deref() == Some("unix") {
            // Termux port: Android has no writable `/tmp`, so Termux:X11's socket lives in
            // `$PREFIX/tmp/.X11-unix`. Try `X11_UNIX_SOCKET_DIR` first, then Termux's prefix,
            // then the usual path.
            #[cfg(feature = "std")]
            {
                let mut dirs: Vec<String> = Vec::new();
                if let Some(dir) = std::env::var_os("X11_UNIX_SOCKET_DIR") {
                    dirs.push(dir.to_string_lossy().into_owned());
                }
                if let Some(prefix) = std::env::var_os("PREFIX") {
                    dirs.push(
                        std::path::PathBuf::from(prefix)
                            .join("tmp/.X11-unix")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                dirs.push(String::from("/tmp/.X11-unix"));
                for dir in dirs {
                    let file_name = format!("{dir}/X{display}");
                    if !targets.contains(&ConnectAddress::Socket(file_name.clone())) {
                        targets.push(ConnectAddress::Socket(file_name));
                    }
                }
            }
            #[cfg(not(feature = "std"))]
            {
                let file_name = format!("/tmp/.X11-unix/X{display}");
                targets.push(ConnectAddress::Socket(file_name));
            }
        }

        if protocol.is_none() && host.is_empty() {
            targets.push(ConnectAddress::Hostname(
                "localhost",
                TCP_PORT_BASE + display,
            ));
        }
    }

    targets.into_iter()
}

#[cfg(all(test, feature = "std"))]
mod tests {
    // make sure iterator properties are clean
    use super::{super::parse_display, ConnectAddress};
    use alloc::{vec, vec::Vec};

    #[test]
    fn basic_test() {
        let pd = parse_display(Some(":0")).unwrap();
        let ci = pd.connect_instruction();
        let ci = ci.collect::<Vec<_>>();

        assert_eq!(
            ci,
            vec![
                ConnectAddress::Socket("/tmp/.X11-unix/X0".into()),
                ConnectAddress::Hostname("localhost", 6000),
            ]
        );
    }

    #[test]
    fn try_over_hostname() {
        let pd = parse_display(Some("192.168.1.111:0")).unwrap();
        let ci = pd.connect_instruction();

        let ci = ci.collect::<Vec<_>>();

        assert_eq!(ci, vec![ConnectAddress::Hostname("192.168.1.111", 6000),]);
    }

    #[test]
    fn try_over_unix_hostname() {
        let pd = parse_display(Some("unix/host:0")).unwrap();
        let ci = pd.connect_instruction();

        let ci = ci.collect::<Vec<_>>();

        assert_eq!(ci, vec![ConnectAddress::Socket("/tmp/.X11-unix/X0".into())]);
    }
}
