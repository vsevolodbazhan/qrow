//! Exercise the pinned driver's local barrier and bounded metadata on the wire.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn frontend(server: &mut tokio::io::DuplexStream) -> u8 {
    let tag = server.read_u8().await.unwrap();
    let len = server.read_u32().await.unwrap() as usize;
    assert!((4..2 * 1024 * 1024).contains(&len));
    server.read_exact(&mut vec![0; len - 4]).await.unwrap();
    tag
}

async fn packet(server: &mut tokio::io::DuplexStream, tag: u8, body: &[u8]) {
    server.write_u8(tag).await.unwrap();
    server.write_u32((body.len() + 4) as u32).await.unwrap();
    server.write_all(body).await.unwrap();
}

fn description(count: u16, name: &str, oid: u32) -> Vec<u8> {
    let mut body = count.to_be_bytes().to_vec();
    for _ in 0..count {
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&oid.to_be_bytes());
        body.extend_from_slice(&(-1i16).to_be_bytes());
        body.extend_from_slice(&(-1i32).to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
    }
    body
}

#[test]
fn buffer_reset_waits_for_ready_and_partial_frames_and_blocks_later_requests() {
    let (session, mut server) = tests::idle_session();
    session.runtime.block_on(async {
        let client = session.client.clone();
        let sql = format!("SELECT 1 /*{}*/", "x".repeat(1024 * 1024));
        let stream = client.simple_query_raw(&sql).await.unwrap();
        pin_mut!(stream);
        let peer = async {
            assert_eq!(frontend(&mut server).await, b'Q');
            packet(&mut server, b'T', &description(1, "value", 25)).await;
            let mut row = 1u16.to_be_bytes().to_vec();
            row.extend_from_slice(&(1024 * 1024i32).to_be_bytes());
            row.extend(vec![b'x'; 1024 * 1024]);
            packet(&mut server, b'D', &row).await;
            packet(&mut server, b'C', b"SELECT 1\0").await;
        };
        let consume = async {
            assert!(matches!(
                stream.next().await.unwrap().unwrap(),
                SimpleQueryMessage::RowDescription(_)
            ));
            let SimpleQueryMessage::Row(row) = stream.next().await.unwrap().unwrap() else {
                panic!("missing row")
            };
            assert_eq!(row.get(0).unwrap().len(), 1024 * 1024);
            drop(row);
            assert!(matches!(
                stream.next().await.unwrap().unwrap(),
                SimpleQueryMessage::CommandComplete(_)
            ));
        };
        tokio::join!(peer, consume);
    });
    // The scope above drops SimpleQueryStream before the local reset request.
    session.runtime.block_on(async {
        let client = session.client.clone();
        let boundary = session.cancel.transport.boundary.clone();
        let reset = tokio::spawn(async move { client.reset_buffers_when_idle(boundary).await });
        tokio::task::yield_now().await;
        let client = session.client.clone();
        let successor = tokio::spawn(async move { client.check_connection().await });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), server.read_u8())
                .await
                .is_err()
        );
        assert!(!reset.is_finished());
        server.write_all(b"Z\0\0").await.unwrap();
        tokio::task::yield_now().await;
        assert!(!session.cancel.transport.boundary.load(Ordering::SeqCst));
        assert!(!reset.is_finished());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), server.read_u8())
                .await
                .is_err()
        );
        server.write_all(b"\0\x05I").await.unwrap();
        let report = tokio::time::timeout(Duration::from_secs(2), reset)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(report.read_before > 512 * 1024, "{report:?}");
        assert!(report.scratch_before >= 1024 * 1024, "{report:?}");
        assert_eq!(
            (report.read_after, report.write_after, report.scratch_after),
            (8192, 8192, 0)
        );
        assert_eq!(frontend(&mut server).await, b'S');
        server.write_all(b"Z\0\0\0\x05I").await.unwrap();
        successor.await.unwrap().unwrap();
    });
}

#[test]
fn bounded_describe_never_requests_recursive_custom_type_metadata() {
    for (count, params, name, valid) in [
        (1, 0, "custom", true),
        (4097, 0, "v", false),
        (1, 0, "", false),
        (1, 4097, "v", false),
    ] {
        let (session, mut server) = tests::idle_session();
        let name = if name.is_empty() {
            "x".repeat(1025)
        } else {
            name.into()
        };
        session.runtime.block_on(async {
            let describe = session.client.prepare_bounded("SELECT custom_value");
            let peer = async {
                while frontend(&mut server).await != b'S' {}
                packet(&mut server, b'1', &[]).await;
                let mut parameters = (params as u16).to_be_bytes().to_vec();
                for _ in 0..params {
                    parameters.extend_from_slice(&23u32.to_be_bytes());
                }
                packet(&mut server, b't', &parameters).await;
                packet(&mut server, b'T', &description(count, &name, 987654)).await;
                server.write_all(b"Z\0\0\0\x05I").await.unwrap();
            };
            let (result, ()) = tokio::join!(describe, peer);
            if valid {
                let statement = result.unwrap();
                assert_eq!(statement.columns()[0].type_().oid(), 987654);
                assert_eq!(statement.columns()[0].type_().name(), "");
                // Retain the statement, so a Close message cannot mask a type query.
                assert!(
                    tokio::time::timeout(Duration::from_millis(30), server.read_u8())
                        .await
                        .is_err()
                );
                drop(statement);
            } else {
                assert!(result.is_err());
                // A rejected description still owns the parsed server statement.
                assert_eq!(frontend(&mut server).await, b'C');
                assert_eq!(frontend(&mut server).await, b'S');
                packet(&mut server, b'3', &[]).await;
                server.write_all(b"Z\0\0\0\x05I").await.unwrap();
            }
        });
    }
}
