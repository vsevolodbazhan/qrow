# Local Postgres demo

Start a disposable Postgres database with synthetic data to try a Qrow build.
Docker Compose must be installed and running.

From the repository root, run:

```sh
./scripts/local-postgres/db up
```

Create a Qrow connection with these settings:

| Setting | Value |
| --- | --- |
| Type | Postgres |
| Host | `127.0.0.1` |
| Port | `55432` |
| Database | `qrow_demo` |
| User | `qrow_demo` |
| Password | `qrow_demo_password` |
| TLS mode | Disabled |

Try `SELECT * FROM order_summary;` or browse the `public` schema. The sample
tables include dates, timestamps, exact decimals, nullable text, booleans,
constraints, joins, and a view.

Use the same command with `down` to stop the server and keep its data. Use
`logs`, `status`, or `query` to inspect it. `reset` removes the demo database
volume and recreates it from the SQL files; this deletes only this fixture's
data. The port is bound to localhost. Do not change the sample password for
real data or expose this server to an untrusted network.

The database initialization scripts run only when Docker creates a new data
volume. Edit the SQL files and run `reset` to apply changes.
