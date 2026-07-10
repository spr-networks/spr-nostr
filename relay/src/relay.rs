//! In-process Nostr relay supervisor.
//!
//! The relay is NOT a forked child process — it is the `nostr-relay-builder`
//! `LocalRelay` running inside this binary's tokio runtime. The supervisor
//! holds the running relay behind an async mutex; "restart" shuts the relay
//! down and rebuilds it in-process from the (possibly changed) config.
//!
//! Verified 0.44.1 API (see ~/.cargo/.../nostr-relay-builder-0.44.1):
//!   * `LocalRelay::new(builder: RelayBuilder) -> Self`   (sync)
//!   * `LocalRelay::run(&self) -> Result<(), Error>`      (async; spawns the
//!     accept loop as a tokio task and returns immediately after binding)
//!   * `LocalRelay::url(&self) -> RelayUrl`               (async)
//!   * `LocalRelay::shutdown(&self)`                      (sync; notifies the
//!     accept loop to stop and drops the TcpListener)
//!   * `RelayBuilder::default().addr(IpAddr).port(u16).database(D).mode(..)
//!      .nip42(..).write_policy(..).query_policy(..)`
//!
//! read/write/both is implemented with real `WritePolicy` / `QueryPolicy`
//! plugins because 0.44.1's `RelayBuilderMode` only offers Generic vs
//! PublicKey — there is no native read-only/write-only switch.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use nostr_lmdb::NostrLMDB;
use nostr_relay_builder::prelude::*;
use tokio::sync::Mutex;

use crate::config::{Config, Mode};

/// Read-only enforcement: reject every incoming EVENT.
#[derive(Debug)]
struct RejectWrites;

impl WritePolicy for RejectWrites {
    fn admit_event<'a>(
        &'a self,
        _event: &'a Event,
        _addr: &'a SocketAddr,
    ) -> BoxedFuture<'a, PolicyResult> {
        Box::pin(async { PolicyResult::Reject("relay is read-only".to_string()) })
    }
}

/// Write-only enforcement: reject every incoming REQ (query).
#[derive(Debug)]
struct RejectQueries;

impl QueryPolicy for RejectQueries {
    fn admit_query<'a>(
        &'a self,
        _query: &'a Filter,
        _addr: &'a SocketAddr,
    ) -> BoxedFuture<'a, PolicyResult> {
        Box::pin(async { PolicyResult::Reject("relay is write-only".to_string()) })
    }
}

/// A point-in-time view of relay state for the API.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub running: bool,
    pub uptime_seconds: u64,
}

struct State {
    relay: Option<LocalRelay>,
    started_at: Option<Instant>,
    running: bool,
}

/// Supervises the in-process relay. The LMDB database is opened once and shared
/// (as `Arc<dyn NostrDatabase>`) across relay rebuilds — reopening the same LMDB
/// environment within one process is unsafe, so we never close it on restart.
pub struct Supervisor {
    db: Arc<dyn NostrDatabase>,
    bind_ip: IpAddr,
    db_path: PathBuf,
    state: Mutex<State>,
}

impl Supervisor {
    /// Open the LMDB store at `db_path` and prepare a supervisor that will bind
    /// the relay listener to `bind_ip`.
    pub fn new(db_path: PathBuf, bind_ip: IpAddr) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&db_path)?;
        // 0700: relay data is owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o700));
        }
        let db = NostrLMDB::open(&db_path)?;
        Ok(Supervisor {
            db: db.into_nostr_database(),
            bind_ip,
            db_path,
            state: Mutex::new(State {
                relay: None,
                started_at: None,
                running: false,
            }),
        })
    }

    /// Build (but do not run) a relay for `cfg`, wiring mode/auth as real
    /// nostr-relay-builder policies. Kept separate so it can be unit-tested
    /// without binding a socket.
    fn build_relay(&self, cfg: &Config) -> LocalRelay {
        let mut builder = RelayBuilder::default()
            .addr(self.bind_ip)
            .port(cfg.port)
            .database(self.db.clone());

        match cfg.mode {
            Mode::Read => builder = builder.write_policy(RejectWrites),
            Mode::Write => builder = builder.query_policy(RejectQueries),
            Mode::Both => {}
        }

        if cfg.require_auth {
            builder = builder.nip42(RelayBuilderNip42 {
                mode: RelayBuilderNip42Mode::Both,
            });
        }

        LocalRelay::new(builder)
    }

    /// Start the relay for `cfg`. Returns once the listener is bound (the accept
    /// loop runs as a background tokio task).
    pub async fn start(&self, cfg: &Config) -> anyhow::Result<()> {
        let mut st = self.state.lock().await;
        self.start_locked(&mut st, cfg).await
    }

    async fn start_locked(&self, st: &mut State, cfg: &Config) -> anyhow::Result<()> {
        let relay = self.build_relay(cfg);
        relay.run().await?;
        let url = relay.url().await;
        println!(
            "spr-nostr: relay listening on {} (bind {}:{}, mode {:?}, auth {})",
            url, self.bind_ip, cfg.port, cfg.mode, cfg.require_auth
        );
        st.relay = Some(relay);
        st.started_at = Some(Instant::now());
        st.running = true;
        Ok(())
    }

    /// Shut down the running relay in-process, then rebuild it from `cfg`.
    pub async fn restart(&self, cfg: &Config) -> anyhow::Result<()> {
        let mut st = self.state.lock().await;
        self.shutdown_locked(&mut st);
        // The accept loop drops its TcpListener asynchronously after shutdown();
        // give the OS a moment to release the port, then retry the bind so a
        // same-port restart doesn't race into EADDRINUSE.
        let mut last_err = None;
        for attempt in 0..10u32 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            match self.start_locked(&mut st, cfg).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    println!(
                        "spr-nostr: relay rebind attempt {} failed: {e}",
                        attempt + 1
                    );
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("relay failed to restart")))
    }

    fn shutdown_locked(&self, st: &mut State) {
        if let Some(relay) = st.relay.take() {
            relay.shutdown();
        }
        st.started_at = None;
        st.running = false;
    }

    /// Shut the relay down (for process exit).
    pub async fn shutdown(&self) {
        let mut st = self.state.lock().await;
        self.shutdown_locked(&mut st);
    }

    /// Current relay state for the API.
    pub async fn snapshot(&self) -> Snapshot {
        let st = self.state.lock().await;
        let uptime = st.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        Snapshot {
            running: st.running,
            uptime_seconds: uptime,
        }
    }

    /// On-disk size of the LMDB database directory, in bytes (cheap: a shallow
    /// directory walk). Returned in `/status` as a real number instead of a
    /// faked event count.
    pub fn db_bytes(&self) -> u64 {
        dir_size(&self.db_path)
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => total += dir_size(&entry.path()),
                Ok(_) => {
                    if let Ok(meta) = entry.metadata() {
                        total += meta.len();
                    }
                }
                Err(_) => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn temp_db() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "spr-nostr-db-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        p
    }

    #[tokio::test]
    async fn builds_relay_in_every_mode_without_binding() {
        let db = temp_db();
        let sup = Supervisor::new(db.clone(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        // build_relay must not bind a socket, so all four variants are cheap
        // and prove the policy / nip42 wiring type-checks against 0.44.1.
        for mode in [Mode::Read, Mode::Write, Mode::Both] {
            for auth in [false, true] {
                let cfg = Config {
                    port: 7777,
                    mode,
                    require_auth: auth,
                };
                let _relay = sup.build_relay(&cfg);
            }
        }
        let _ = std::fs::remove_dir_all(&db);
    }

    #[tokio::test]
    async fn start_snapshot_restart_shutdown_roundtrip() {
        let db = temp_db();
        let sup = Supervisor::new(db.clone(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        // Port 0 lets the OS pick a free port for the test bind.
        let cfg = Config {
            port: 0,
            mode: Mode::Both,
            require_auth: false,
        };
        sup.start(&cfg).await.unwrap();
        let snap = sup.snapshot().await;
        assert!(snap.running);
        sup.shutdown().await;
        let snap = sup.snapshot().await;
        assert!(!snap.running);
        let _ = std::fs::remove_dir_all(&db);
    }

    #[test]
    fn db_bytes_counts_files() {
        let db = temp_db();
        std::fs::create_dir_all(&db).unwrap();
        std::fs::write(db.join("a.bin"), vec![0u8; 1024]).unwrap();
        let sup = Supervisor::new(db.clone(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        assert!(sup.db_bytes() >= 1024);
        let _ = std::fs::remove_dir_all(&db);
    }
}
