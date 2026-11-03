# Qrow HTTP transport patch

This copy keeps reqwest 0.12.28 and its upstream licenses. Qrow uses it to
close a Trino socket during TLS or a stalled response. The normal reqwest
connection path stays the same when TCP ownership is not configured.

`ClientBuilder::tcp_connection_control` calls the owner after TCP connects
and before Rustls starts. The returned lease stays in the connecting future,
then in the pooled connection. An error rejects the socket before TLS or
HTTP authentication. This option requires direct HTTP or Rustls transport.
The builder rejects proxies, local sockets and native TLS for this option.
Hostname checks, ALPN and connection metadata use the upstream TLS rules.

`ClientBuilder::http1_max_buf_size` sets hyper's existing HTTP/1 read-buffer
limit. The upstream default stays the same when this option is absent.
Values below 8192 bytes return a builder error.

Compare changes in `src/connect.rs` and `src/async_impl/client.rs` with the
pinned upstream release. Run the Trino transport unit tests and the Trino
Docker suite through `./qtest` after an upgrade. Keep TCP ownership before
TLS and for pooled connections. Remove this patch when upstream provides
these controls, or apply it to the new version again.

Plan: `.plans/data-export.md`, revision 9, phase 7c.
