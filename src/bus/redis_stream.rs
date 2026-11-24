use super::{BATCH, Batch, BusFuture, Entry, EventBus};
use anyhow::{Context, Result, ensure};
use redis::{
    Client, Cmd, FromRedisValue, RedisResult,
    aio::MultiplexedConnection,
    streams::{StreamAutoClaimReply, StreamReadReply},
};
use std::time::Duration;
use tokio::sync::Mutex;

pub struct RedisBus {
    client: Client,
    connection: Mutex<Option<MultiplexedConnection>>,
    stream: String,
    reclaim_idle: Duration,
}

impl RedisBus {
    pub fn new(url: &str, stream: &str) -> Result<Self> {
        ensure!(!stream.trim().is_empty(), "empty Redis stream key");
        Ok(Self {
            client: Client::open(url)?,
            connection: Mutex::new(None),
            stream: stream.into(),
            reclaim_idle: Duration::from_secs(10),
        })
    }

    async fn query<T: FromRedisValue>(&self, command: &Cmd) -> RedisResult<T> {
        // Share the network connection, but never hold the lock across a command.
        let mut connection = {
            let mut slot = self.connection.lock().await;
            if slot.is_none() {
                let config = redis::AsyncConnectionConfig::new()
                    .set_connection_timeout(Duration::from_secs(3))
                    .set_response_timeout(Duration::from_secs(3));
                *slot = Some(
                    self.client
                        .get_multiplexed_async_connection_with_config(&config)
                        .await?,
                );
            }
            slot.as_ref().expect("connection initialized").clone()
        };
        let result = command.query_async(&mut connection).await;
        if result.as_ref().is_err_and(|e| e.is_io_error()) {
            *self.connection.lock().await = None;
        }
        result
    }

    async fn claim(
        &self,
        group: &str,
        worker: &str,
        cursor: &str,
    ) -> RedisResult<StreamAutoClaimReply> {
        let mut claim = redis::cmd("XAUTOCLAIM");
        claim
            .arg(&self.stream)
            .arg(group)
            .arg(worker)
            .arg(self.reclaim_idle.as_millis() as u64)
            .arg(cursor)
            .arg("COUNT")
            .arg(BATCH);
        match self.query(&claim).await {
            Err(err) if err.code() == Some("NOGROUP") => {
                let mut create = redis::cmd("XGROUP");
                create
                    .arg("CREATE")
                    .arg(&self.stream)
                    .arg(group)
                    .arg("0-0")
                    .arg("MKSTREAM");
                match self.query::<()>(&create).await {
                    Err(err) if err.code() == Some("BUSYGROUP") => (),
                    result => result?,
                }
                self.query(&claim).await
            }
            result => result,
        }
    }
}

impl EventBus for RedisBus {
    fn publish<'a>(&'a self, event_id: &'a str) -> BusFuture<'a, ()> {
        Box::pin(async move {
            let mut cmd = redis::cmd("XADD");
            cmd.arg(&self.stream).arg("*").arg("event_id").arg(event_id);
            self.query::<String>(&cmd).await?;
            Ok(())
        })
    }

    fn receive<'a>(
        &'a self,
        group: &'a str,
        worker: &'a str,
        cursor: &'a str,
    ) -> BusFuture<'a, Batch> {
        Box::pin(async move {
            let claimed = self.claim(group, worker, cursor).await?;
            let mut cmd = redis::cmd("XREADGROUP");
            cmd.arg("GROUP")
                .arg(group)
                .arg(worker)
                .arg("COUNT")
                .arg(BATCH)
                .arg("STREAMS")
                .arg(&self.stream)
                .arg(">");
            let fresh: Option<StreamReadReply> = self.query(&cmd).await?;
            let mut entries = Vec::new();
            let fresh_ids = fresh
                .into_iter()
                .flat_map(|reply| reply.keys)
                .flat_map(|key| key.ids);
            for row in claimed.claimed.into_iter().chain(fresh_ids) {
                let event_id = row
                    .get("event_id")
                    .context("stream entry has no event_id")?;
                entries.push(Entry {
                    receipt: row.id,
                    event_id,
                });
            }
            Ok(Batch {
                entries,
                next_pending_cursor: claimed.next_stream_id,
            })
        })
    }

    fn ack<'a>(&'a self, group: &'a str, receipt: &'a str) -> BusFuture<'a, ()> {
        Box::pin(async move {
            let mut cmd = redis::cmd("XACK");
            cmd.arg(&self.stream).arg(group).arg(receipt);
            self.query::<usize>(&cmd).await?;
            Ok(())
        })
    }
}
