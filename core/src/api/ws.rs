//! WebSocket resync protocol: snapshot on connect, sequenced deltas, drop-oldest buffer.

use std::collections::VecDeque;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use super::events::{EventPublisher, WsBody, WsClientMessage, WsMessage, WS_OUTBOUND_CAPACITY};
use super::SharedManager;

/// Drive one WS client: initial snapshot, then sequenced deltas.
///
/// A slow client cannot back up the daemon — outbound messages sit in a bounded
/// queue that **drops the oldest** entry when full. Sequence gaps (from drops or
/// `broadcast` lag) trigger an automatic resync snapshot; the UI may also send
/// `{ "type": "resync" }`.
pub async fn handle_ws(socket: WebSocket, mgr: SharedManager) {
    let publisher = {
        let m = mgr.read().await;
        m.events.clone()
    };
    let mut bus_rx = publisher.subscribe();

    let (mut sink, mut stream) = socket.split();
    let mut outbound: VecDeque<String> = VecDeque::with_capacity(WS_OUTBOUND_CAPACITY);
    let mut expected_seq = publisher.current_seq().saturating_add(1);

    enqueue_snapshot(&mgr, &publisher, &mut outbound, &mut expected_seq).await;

    loop {
        // Flush one queued message when possible without starving the bus.
        if !outbound.is_empty() {
            tokio::select! {
                biased;

                result = sink.send(Message::Text(outbound.front().unwrap().clone().into())) => {
                    match result {
                        Ok(()) => {
                            outbound.pop_front();
                        }
                        Err(_) => break,
                    }
                }

                event = bus_rx.recv() => {
                    if !handle_bus_event(
                        event,
                        &mgr,
                        &publisher,
                        &mut outbound,
                        &mut expected_seq,
                    )
                    .await
                    {
                        break;
                    }
                }

                msg = stream.next() => {
                    if !handle_client_msg(
                        msg,
                        &mgr,
                        &publisher,
                        &mut outbound,
                        &mut expected_seq,
                    )
                    .await
                    {
                        break;
                    }
                }
            }
        } else {
            tokio::select! {
                event = bus_rx.recv() => {
                    if !handle_bus_event(
                        event,
                        &mgr,
                        &publisher,
                        &mut outbound,
                        &mut expected_seq,
                    )
                    .await
                    {
                        break;
                    }
                }

                msg = stream.next() => {
                    if !handle_client_msg(
                        msg,
                        &mgr,
                        &publisher,
                        &mut outbound,
                        &mut expected_seq,
                    )
                    .await
                    {
                        break;
                    }
                }
            }
        }
    }
}

async fn handle_bus_event(
    event: Result<WsMessage, RecvError>,
    mgr: &SharedManager,
    publisher: &EventPublisher,
    outbound: &mut VecDeque<String>,
    expected_seq: &mut u64,
) -> bool {
    match event {
        Ok(msg) => {
            if msg.seq < *expected_seq {
                return true;
            }
            if msg.seq > *expected_seq {
                warn!(
                    "WS seq gap: expected {}, got {} — resyncing",
                    *expected_seq, msg.seq
                );
                enqueue_snapshot(mgr, publisher, outbound, expected_seq).await;
                return true;
            }
            *expected_seq = msg.seq.saturating_add(1);
            push_drop_oldest(outbound, &msg);
            true
        }
        Err(RecvError::Lagged(n)) => {
            warn!("WS broadcast lagged by {n} — resyncing");
            enqueue_snapshot(mgr, publisher, outbound, expected_seq).await;
            true
        }
        Err(RecvError::Closed) => false,
    }
}

async fn handle_client_msg(
    msg: Option<Result<Message, axum::Error>>,
    mgr: &SharedManager,
    publisher: &EventPublisher,
    outbound: &mut VecDeque<String>,
    expected_seq: &mut u64,
) -> bool {
    match msg {
        None | Some(Err(_)) => false,
        Some(Ok(Message::Close(_))) => false,
        Some(Ok(Message::Text(text))) => {
            if let Ok(client) = serde_json::from_str::<WsClientMessage>(text.as_str()) {
                match client {
                    WsClientMessage::Resync => {
                        debug!("WS client requested resync");
                        enqueue_snapshot(mgr, publisher, outbound, expected_seq).await;
                    }
                    WsClientMessage::Ping => {}
                }
            }
            true
        }
        Some(Ok(_)) => true,
    }
}

async fn enqueue_snapshot(
    mgr: &SharedManager,
    publisher: &EventPublisher,
    outbound: &mut VecDeque<String>,
    expected_seq: &mut u64,
) {
    let jobs = {
        let m = mgr.read().await;
        m.list_jobs().into_iter().cloned().collect()
    };
    let snap = publisher.snapshot(jobs);
    *expected_seq = snap.seq.saturating_add(1);
    outbound.clear();
    push_drop_oldest(outbound, &snap);
}

fn push_drop_oldest(outbound: &mut VecDeque<String>, msg: &WsMessage) {
    // Collapse pending segment ticks for the same job+segment so a slow UI
    // never queues hundreds of obsolete progress frames.
    if let WsBody::SegmentProgress {
        job_id,
        segment_id,
        ..
    } = &msg.body
    {
        outbound.retain(|raw| {
            match serde_json::from_str::<WsMessage>(raw) {
                Ok(prev) => !matches!(
                    &prev.body,
                    WsBody::SegmentProgress {
                        job_id: j,
                        segment_id: s,
                        ..
                    } if j == job_id && s == segment_id
                ),
                Err(_) => true,
            }
        });
    } else if matches!(&msg.body, WsBody::JobUpdated { .. }) {
        if let WsBody::JobUpdated { job } = &msg.body {
            let id = job.id;
            outbound.retain(|raw| {
                match serde_json::from_str::<WsMessage>(raw) {
                    Ok(prev) => !matches!(
                        &prev.body,
                        WsBody::JobUpdated { job: j } if j.id == id
                    ),
                    Err(_) => true,
                }
            });
        }
    }

    let Ok(json) = serde_json::to_string(msg) else {
        return;
    };
    while outbound.len() >= WS_OUTBOUND_CAPACITY {
        let _ = outbound.pop_front();
    }
    outbound.push_back(json);
}
