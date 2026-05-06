//! Thin builder helpers that hide the renet/netcode boilerplate
//! behind one-liners both ends can call. The actual `update` /
//! `recv_message` / `send_message` calls happen in the game crate;
//! this module only handles construction and key derivation.
//!
//! ## Authentication (Phase 1)
//!
//! Both endpoints use [`ClientAuthentication::Unsecure`] /
//! [`ServerAuthentication::Unsecure`] for now. That's fine while we
//! tunnel localhost between two machines and have no real account
//! identity. Once the auth service exists (Phase 5+) we'll switch to
//! signed connect tokens issued by it.

use std::{
    net::{SocketAddr, UdpSocket},
    time::SystemTime,
};

use renet::transport::{
    ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication,
    ServerConfig,
};
use renet::{RenetClient, RenetServer};

use crate::{ids::ClientId, protocol::NetSettings, PROTOCOL_ID};

/// Pair of (renet client + udp/netcode transport) returned by
/// [`open_client`]. Both must be polled together by the caller every
/// frame:
///
/// ```ignore
/// transport.update(dt, &mut client)?;
/// // ... read messages, apply, send messages ...
/// transport.send_packets(&mut client)?;
/// ```
pub struct ClientHandle {
    pub client: RenetClient,
    pub transport: NetcodeClientTransport,
}

/// Pair of (renet server + udp/netcode transport).
pub struct ServerHandle {
    pub server: RenetServer,
    pub transport: NetcodeServerTransport,
}

/// Open a client endpoint and start the connect handshake against
/// `server_addr`. The returned [`RenetClient`] is in
/// [`RenetConnectionStatus::Connecting`](renet::RenetConnectionStatus)
/// until netcode finishes; callers should poll `client.is_connected()`
/// before sending [`crate::ClientMsg::Hello`].
pub fn open_client(
    server_addr: SocketAddr,
    client_id: ClientId,
    settings: &NetSettings,
) -> std::io::Result<ClientHandle> {
    // Bind to any free port on all interfaces. The OS picks the
    // ephemeral port; netcode wraps the socket and handles all
    // packet-level work after this.
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time before unix epoch");

    let auth = ClientAuthentication::Unsecure {
        protocol_id: PROTOCOL_ID,
        client_id: client_id.0,
        server_addr,
        // No pre-shared user data in Phase 1. Once we wire the auth
        // service this will carry an HMAC of the player's session
        // token so the server can identify which account is joining.
        user_data: None,
    };

    let transport = NetcodeClientTransport::new(current_time, auth, socket).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("netcode init: {e}"))
    })?;
    let client = RenetClient::new(settings.to_renet());

    log::info!(
        "rift-net: client opened, target={} client_id={}",
        server_addr,
        client_id.0
    );
    Ok(ClientHandle { client, transport })
}

/// Open a server endpoint listening on `bind_addr`. `public_addr` is
/// the address clients are told to use in their connect token — for
/// localhost development they're the same; behind NAT or a proxy
/// they'll diverge.
pub fn open_server(
    bind_addr: SocketAddr,
    public_addr: SocketAddr,
    max_clients: usize,
    settings: &NetSettings,
) -> std::io::Result<ServerHandle> {
    let socket = UdpSocket::bind(bind_addr)?;
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time before unix epoch");

    let server_config = ServerConfig {
        current_time,
        max_clients,
        protocol_id: PROTOCOL_ID,
        public_addresses: vec![public_addr],
        // Unsecure for Phase 1. We'll graduate to
        // `ServerAuthentication::Secure { private_key }` once the
        // auth service issues signed tokens.
        authentication: ServerAuthentication::Unsecure,
    };
    let transport = NetcodeServerTransport::new(server_config, socket)?;
    let server = RenetServer::new(settings.to_renet());

    log::info!(
        "rift-net: server opened, bind={} public={} max_clients={}",
        bind_addr,
        public_addr,
        max_clients
    );
    Ok(ServerHandle { server, transport })
}
