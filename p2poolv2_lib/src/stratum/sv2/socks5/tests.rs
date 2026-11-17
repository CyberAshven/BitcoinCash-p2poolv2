use super::*;
use crate::test_utils::TestSocksProxy;
use tokio::io::duplex;
use tokio::net::TcpListener;

const ONION: &str = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuvwx.onion";

/// A name goes to the proxy as a name, with its port: this computer never
/// resolves it. Each connection sends its own username, so Tor gives each
/// its own circuit. The bytes after the proxy's reply are the target's.
#[tokio::test]
async fn a_name_is_sent_to_the_proxy_unresolved_with_a_username_of_its_own() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut proxy = TestSocksProxy::start(Some(target.local_addr().unwrap()), true).await;
    let mut usernames = Vec::with_capacity(2);
    let onion = format!("{ONION}:3335");
    for _ in 0..2 {
        let (stream, accepted) = tokio::join!(connect(&proxy.address, &onion), target.accept());
        let mut stream = stream.unwrap();
        let (mut served, _) = accepted.unwrap();
        served.write_all(b"hello").await.unwrap();
        let mut first = [0u8; 5];
        stream.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"hello");
        let request = proxy.next_request().await;
        assert_eq!(request.address_type, ADDRESS_NAME);
        assert_eq!(request.host, ONION.as_bytes());
        assert_eq!(request.port, 3335);
        usernames.push(request.username.unwrap());
    }
    assert_ne!(usernames[0], usernames[1]);
}

/// A proxy without username/password, such as an I2P router's, is used
/// with no authentication, and a name in its reply is read off the stream.
#[tokio::test]
async fn a_proxy_without_usernames_is_used_and_a_name_in_its_reply_is_skipped() {
    let (mut client, mut proxy) = duplex(256);
    let serve = async {
        let mut greeting = [0u8; 4];
        proxy.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [5, 2, 0, 2]);
        proxy.write_all(&[5, 0]).await.unwrap();
        let mut request = vec![0u8; 5 + 7 + 2];
        proxy.read_exact(&mut request).await.unwrap();
        assert_eq!(&request[..5], &[5, 1, 0, 3, 7]);
        assert_eq!(&request[5..12], b"abc.i2p");
        assert_eq!(&request[12..], &[0x0d, 0x07]);
        proxy
            .write_all(&[5, 0, 0, 3, 2, b'o', b'k', 0, 0])
            .await
            .unwrap();
        proxy.write_all(b"X").await.unwrap();
    };
    let (negotiated, ()) = tokio::join!(negotiate(&mut client, "abc.i2p", 3335), serve);
    negotiated.unwrap();
    let mut next = [0u8; 1];
    client.read_exact(&mut next).await.unwrap();
    assert_eq!(&next, b"X");
}

#[test]
fn an_ip_address_is_sent_as_its_bytes() {
    assert_eq!(
        connect_request("192.0.2.7", 3335).unwrap(),
        [5, 1, 0, 1, 192, 0, 2, 7, 0x0d, 0x07]
    );
    let ipv6 = connect_request("2001:db8::1", 1).unwrap();
    assert_eq!(&ipv6[..4], &[5, 1, 0, 4]);
    assert_eq!(ipv6.len(), 4 + 16 + 2);
    assert_eq!(
        split_host_port("[2001:db8::1]:1").unwrap(),
        ("2001:db8::1", 1)
    );
    assert!(connect_request(&"a".repeat(256), 1).is_err());
}

/// A proxy that cannot reach the target fails the dial with its reason.
#[tokio::test]
async fn a_refusal_by_the_proxy_is_an_error_with_its_reason() {
    let proxy = TestSocksProxy::start(None, false).await;
    let refused = connect(&proxy.address, &format!("{ONION}:3335"))
        .await
        .unwrap_err();
    assert!(refused.contains("connection refused"), "{refused}");
    assert!(refused.contains(ONION), "{refused}");
}

#[tokio::test]
async fn a_proxy_that_accepts_no_offered_method_is_an_error() {
    let (mut client, mut proxy) = duplex(64);
    let serve = async {
        let mut greeting = [0u8; 4];
        proxy.read_exact(&mut greeting).await.unwrap();
        proxy.write_all(&[5, 0xff]).await.unwrap();
    };
    let (negotiated, ()) = tokio::join!(negotiate(&mut client, "abc.onion", 1), serve);
    assert!(negotiated.unwrap_err().contains("accepts neither"));
}

#[test]
fn only_an_i2p_name_is_an_i2p_host() {
    assert!(is_i2p_host("abcdefghijklmnop.b32.i2p"));
    assert!(is_i2p_host("POOL.I2P"));
    assert!(!is_i2p_host(ONION));
    assert!(!is_i2p_host("i2p.example.org"));
}
