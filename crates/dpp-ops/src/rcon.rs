use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

const TYPE_AUTH: i32 = 3;
const TYPE_AUTH_RESPONSE: i32 = 2;
const TYPE_COMMAND: i32 = 2;
const MAX_PACKET_BODY: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RconConfig {
    pub host: String,
    pub port: u16,
    pub password: String,
}

/// Read the minimal RCON settings from `server.properties`.
///
/// The password is read from `DPP_RCON_PASSWORD`; it is never read from the
/// property file, written to state, or printed.
pub fn config_from_server(root: &Path) -> Result<RconConfig> {
    let path = root.join("server.properties");
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let enabled = property(&text, "enable-rcon").unwrap_or("false");
    if !enabled.eq_ignore_ascii_case("true") {
        bail!("RCON is not enabled in {}", path.display());
    }
    let port = property(&text, "rcon.port")
        .unwrap_or("25575")
        .parse::<u16>()
        .context("invalid rcon.port in server.properties")?;
    let password =
        std::env::var("DPP_RCON_PASSWORD").context("DPP_RCON_PASSWORD is required for --rcon")?;
    if password.is_empty() {
        bail!("DPP_RCON_PASSWORD must not be empty");
    }
    Ok(RconConfig {
        host: "127.0.0.1".to_owned(),
        port,
        password,
    })
}

fn property<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then_some(value.trim())
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadReport {
    pub enabled: String,
    pub missing: Vec<String>,
}

pub fn reload_and_list(config: &RconConfig, expected: &[String]) -> Result<ReloadReport> {
    let mut client = RconClient::connect(config)?;
    client.authenticate(&config.password)?;
    client.command("reload")?;
    let enabled = client.command("datapack list enabled")?;
    let missing = expected
        .iter()
        .filter(|name| {
            !enabled.contains(&format!("file/{name}")) && !enabled.contains(name.as_str())
        })
        .cloned()
        .collect();
    Ok(ReloadReport { enabled, missing })
}

struct RconClient {
    stream: TcpStream,
}

impl RconClient {
    fn connect(config: &RconConfig) -> Result<Self> {
        let address = format!("{}:{}", config.host, config.port);
        let socket = address
            .to_socket_addrs()
            .with_context(|| format!("failed to resolve {address}"))?
            .next()
            .context("RCON address resolved to no socket")?;
        let stream = TcpStream::connect_timeout(&socket, Duration::from_secs(5))
            .with_context(|| format!("failed to connect to RCON at {address}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .context("failed to set RCON read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .context("failed to set RCON write timeout")?;
        Ok(Self { stream })
    }

    fn authenticate(&mut self, password: &str) -> Result<()> {
        self.write_packet(1, TYPE_AUTH, password.as_bytes())?;
        let (id, kind, _) = self.read_packet()?;
        if id == -1 || kind != TYPE_AUTH_RESPONSE {
            bail!("RCON authentication failed");
        }
        Ok(())
    }

    fn command(&mut self, command: &str) -> Result<String> {
        self.write_packet(2, TYPE_COMMAND, command.as_bytes())?;
        let (id, _kind, body) = self.read_packet()?;
        if id != 2 {
            bail!("RCON returned an unexpected request id");
        }
        String::from_utf8(body).context("RCON response is not valid UTF-8")
    }

    fn write_packet(&mut self, id: i32, kind: i32, body: &[u8]) -> Result<()> {
        let length = i32::try_from(body.len() + 10).context("RCON packet is too large")?;
        self.stream.write_all(&length.to_le_bytes())?;
        self.stream.write_all(&id.to_le_bytes())?;
        self.stream.write_all(&kind.to_le_bytes())?;
        self.stream.write_all(body)?;
        self.stream.write_all(&[0, 0])?;
        self.stream.flush()?;
        Ok(())
    }

    fn read_packet(&mut self) -> Result<(i32, i32, Vec<u8>)> {
        let mut length = [0u8; 4];
        self.stream.read_exact(&mut length)?;
        let length = i32::from_le_bytes(length);
        if !(10..=MAX_PACKET_BODY as i32 + 10).contains(&length) {
            bail!("invalid RCON packet length {length}");
        }
        let mut packet = vec![0u8; length as usize];
        self.stream.read_exact(&mut packet)?;
        let id = i32::from_le_bytes(packet[0..4].try_into().expect("packet id"));
        let kind = i32::from_le_bytes(packet[4..8].try_into().expect("packet type"));
        let body_end = packet.len().saturating_sub(2);
        Ok((id, kind, packet[8..body_end].to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn parses_properties() {
        let text = "enable-rcon=true\nrcon.port=25565\nrcon.password=ignored\n";
        assert_eq!(property(text, "enable-rcon"), Some("true"));
        assert_eq!(property(text, "rcon.port"), Some("25565"));
        assert_eq!(property(text, "missing"), None);
    }

    #[test]
    fn authenticates_reloads_and_checks_enabled_packs() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (id, kind, body) = read_packet(&mut stream);
            assert_eq!((id, kind), (1, TYPE_AUTH));
            assert_eq!(body, b"secret");
            write_packet(&mut stream, 1, TYPE_AUTH_RESPONSE, b"");

            let (id, kind, body) = read_packet(&mut stream);
            assert_eq!((id, kind), (2, TYPE_COMMAND));
            assert_eq!(body, b"reload");
            write_packet(&mut stream, 2, 0, b"ok");

            let (id, kind, body) = read_packet(&mut stream);
            assert_eq!((id, kind), (2, TYPE_COMMAND));
            assert_eq!(body, b"datapack list enabled");
            write_packet(
                &mut stream,
                2,
                0,
                b"There are 1 data packs enabled: [file/0050_demo]",
            );
        });

        let config = RconConfig {
            host: "127.0.0.1".to_owned(),
            port: address.port(),
            password: "secret".to_owned(),
        };
        let report = reload_and_list(&config, &["0050_demo".to_owned()]).unwrap();
        assert!(report.missing.is_empty());
        server.join().unwrap();
    }

    fn read_packet(stream: &mut TcpStream) -> (i32, i32, Vec<u8>) {
        let mut length = [0u8; 4];
        stream.read_exact(&mut length).unwrap();
        let mut packet = vec![0u8; i32::from_le_bytes(length) as usize];
        stream.read_exact(&mut packet).unwrap();
        let id = i32::from_le_bytes(packet[0..4].try_into().unwrap());
        let kind = i32::from_le_bytes(packet[4..8].try_into().unwrap());
        (id, kind, packet[8..packet.len().saturating_sub(2)].to_vec())
    }

    fn write_packet(stream: &mut TcpStream, id: i32, kind: i32, body: &[u8]) {
        let length = (body.len() + 10) as i32;
        stream.write_all(&length.to_le_bytes()).unwrap();
        stream.write_all(&id.to_le_bytes()).unwrap();
        stream.write_all(&kind.to_le_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.write_all(&[0, 0]).unwrap();
    }
}
