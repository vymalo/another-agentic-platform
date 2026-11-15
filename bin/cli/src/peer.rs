//! Who owns the other end of a connection to the sign-in listener (Linux).
//!
//! A TCP connection on the loopback has no `SO_PEERCRED`, but the kernel lists every IPv4 socket of the network namespace in
//! `/proc/net/tcp` with the uid of its owner. The client's own socket is the row whose `local_address` is the peer's address and port
//! and whose `rem_address` is the listener's; its `uid` column is who made the connection. [`owner_in`] reads that text and nothing
//! else: a pure function, so its edge cases are unit tests.
//!
//! The columns are `sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode ...`. An address is
//! eight hex digits holding the four bytes in memory order (so `0100007F` is `127.0.0.1` on a little-endian machine), a colon and
//! a four-digit hex port. A row that does not parse that way is not a row: it matches nothing (and a longer address, as an
//! IPv4-mapped IPv6 one would be, is such a row).

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

/// The file that lists the IPv4 TCP sockets.
#[cfg(target_os = "linux")]
const TABLE: &str = "/proc/net/tcp";

/// What the table says of a connection.
#[derive(Debug, PartialEq, Eq)]
pub enum Owner {
    /// The uid of the process that owns the client socket.
    Uid(u32),
    /// No row of the table is that connection.
    Unknown,
}

fn address(text: &str) -> Option<SocketAddrV4> {
    let (ip, port) = text.split_once(':')?;
    if ip.len() != 8 || port.len() != 4 {
        return None;
    }
    if !ip
        .bytes()
        .chain(port.bytes())
        .all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let ip = u32::from_str_radix(ip, 16).ok()?;
    let port = u16::from_str_radix(port, 16).ok()?;
    Some(SocketAddrV4::new(Ipv4Addr::from(ip.to_ne_bytes()), port))
}

/// The uid that owns the socket whose local end is `client` and whose remote end is `listener`, read from the text of
/// `/proc/net/tcp`. A row that is not well formed is skipped; the header is the first line.
pub fn owner_in(table: &str, client: SocketAddrV4, listener: SocketAddrV4) -> Owner {
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // sl, local, remote, st, tx:rx, tr:tm, retrnsmt, uid, timeout, inode, ...
        if fields.len() < 10 {
            continue;
        }
        let (Some(local), Some(remote)) = (address(fields[1]), address(fields[2])) else {
            continue;
        };
        if local != client || remote != listener {
            continue;
        }
        if let Ok(uid) = fields[7].parse::<u32>() {
            return Owner::Uid(uid);
        }
    }
    Owner::Unknown
}

/// Whether this system lets the check be made: the table can be read. Decided once, at the start of a sign-in.
pub fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(TABLE).is_ok_and(|t| t.lines().next().is_some())
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// The effective uid of this process: the only one whose connections the listener serves.
pub fn own_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Who owns the client end of a connection that was just accepted. The row is looked for, and once more after 20 ms when it is not
/// there yet.
pub async fn owner_of(client: SocketAddrV4, listener: SocketAddrV4) -> Owner {
    for attempt in 0..2 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let Ok(Some(table)) = tokio::task::spawn_blocking(read_table).await else {
            return Owner::Unknown;
        };
        if let Owner::Uid(uid) = owner_in(&table, client, listener) {
            return Owner::Uid(uid);
        }
    }
    Owner::Unknown
}

fn read_table() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(TABLE).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(all(test, target_endian = "little"))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode";
    // A listener on 127.0.0.1:5433 (0x1539), a client from 127.0.0.1:40000 (0x9C40) owned by uid 1000, the server's end of it,
    // an unrelated socket of uid 0, and an IPv6-mapped row.
    const ROWS: &str = "   0: 0100007F:1539 00000000:0000 0A 00000000:00000000 00:00000000 00000000   102        0 1080384 1 000000007b04819f 100 0 0 10 0
   1: 0100007F:9C40 0100007F:1539 01 00000000:00000000 00:00000000 00000000  1000        0 2222222 1 0000000000000000 20 4 30 10 -1
   2: 0100007F:1539 0100007F:9C40 01 00000000:00000000 00:00000000 00000000   102        0 3333333 1 0000000000000000 20 4 30 10 -1
   3: 0100007F:9C41 0100007F:1539 01 00000000:00000000 00:00000000 00000000     0        0 4444444 1 0000000000000000 20 4 30 10 -1";

    fn table(rows: &str) -> String {
        format!("{HEADER}\n{rows}\n")
    }

    fn client(port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
    }

    fn listener() -> SocketAddrV4 {
        client(5433)
    }

    #[test]
    fn the_row_of_the_client_socket_names_its_owner() {
        let t = table(ROWS);
        assert_eq!(owner_in(&t, client(40000), listener()), Owner::Uid(1000));
        assert_eq!(owner_in(&t, client(40001), listener()), Owner::Uid(0));
    }

    #[test]
    fn the_servers_own_end_of_the_connection_is_not_the_clients_row() {
        // The row whose local end is the listener's is the server's socket (uid 102 here, whoever runs the listener): looking for the
        // client's address with the ends the wrong way round finds that row, which is why the order of the two addresses matters.
        let t = table(ROWS);
        assert_eq!(owner_in(&t, listener(), client(40000)), Owner::Uid(102));
        assert_eq!(owner_in(&t, client(40000), client(40000)), Owner::Unknown);
    }

    #[test]
    fn another_port_or_no_row_is_unknown() {
        let t = table(ROWS);
        assert_eq!(owner_in(&t, client(40002), listener()), Owner::Unknown);
        assert_eq!(owner_in(&t, client(40000), client(5434)), Owner::Unknown);
        assert_eq!(
            owner_in(&table(""), client(40000), listener()),
            Owner::Unknown
        );
        assert_eq!(owner_in("", client(40000), listener()), Owner::Unknown);
    }

    #[test]
    fn a_malformed_row_matches_nothing_and_does_not_stop_the_search() {
        let rows = format!(
            "   9: 0100007F:9C40 0100007F:1539 01\n  10: 0100007F:9C40 0100007F:1539 01 00000000:00000000 00:00000000 00000000  NOPE        0 1 1 0 0 0 0 0\n  11: 0100007Z:9C40 0100007F:1539 01 00000000:00000000 00:00000000 00000000  7        0 1 1 0 0 0 0 0\n  12: 0100007F:9C4 0100007F:1539 01 00000000:00000000 00:00000000 00000000  8        0 1 1 0 0 0 0 0\n{ROWS}"
        );
        assert_eq!(
            owner_in(&table(&rows), client(40000), listener()),
            Owner::Uid(1000)
        );
        let only_bad = "   9: 0100007F:9C40 0100007F:1539 01\n  10: 0100007F:9C40 0100007F:1539 01 00000000:00000000 00:00000000 00000000  NOPE        0 1 1 0 0 0 0 0";
        assert_eq!(
            owner_in(&table(only_bad), client(40000), listener()),
            Owner::Unknown
        );
    }

    #[test]
    fn an_ipv4_mapped_row_does_not_match() {
        // `/proc/net/tcp6` writes 32 hex digits; a row like it in this table is not an IPv4 row, and 127.0.0.1 is not matched through it.
        let mapped = "   1: 0000000000000000FFFF00000100007F:9C40 0000000000000000FFFF00000100007F:1539 01 00000000:00000000 00:00000000 00000000  1000        0 1 1 0 0 0 0 0";
        assert_eq!(
            owner_in(&table(mapped), client(40000), listener()),
            Owner::Unknown
        );
    }

    #[test]
    fn an_address_is_four_bytes_in_memory_order_and_a_port() {
        assert_eq!(
            address("0100007F:1539"),
            Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 5433))
        );
        for bad in [
            "",
            "0100007F",
            "0100007F:",
            ":1539",
            "0100007F:153",
            "0100007F:15390",
            "0100007FF:1539",
            "+100007F:1539",
        ] {
            assert_eq!(address(bad), None, "{bad}");
        }
    }
}
